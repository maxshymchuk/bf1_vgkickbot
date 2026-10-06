extern crate core;

mod api;
mod botstatus;
mod calibration;
mod config;
mod console;
mod cycle;
mod discord;
mod errors;
mod failure;
mod recognition;

use crate::api::bf1api::server::ServerDetails;
use crate::api::bf1api::BF1Api;
use crate::botstatus::{BotStatus, StatusTypes};
use crate::config::{
    load_kick_history_record, save_kick_record, Config, PlayerKickHistoryRecord, StartupConfig,
};
use crate::console::{clear, log, update_status};
use crate::cycle::{execute, Executors, GameState, SpecCycle};
use crate::discord::{announce_bot_crashed, announce_monitoring, announce_shutdown};
use crate::errors::KickbotError;
use crate::errors::KickbotError::ScreenshotError;
use crate::recognition::kick_player::kick_player;
use crate::recognition::model::Classifier;
use chrono::{DateTime, Utc};
use crossterm::event::{poll, read, Event};
use enigo::Direction::{Press, Release};
use enigo::{Button, Enigo, Keyboard, Mouse, Settings};
use serenity::all::MemberAction::Kick;
use std::collections::HashSet;
use std::ffi::CString;
use std::io::ErrorKind;
use std::ops::Deref;
use std::path::Path;
use std::process::{exit, Command, ExitCode};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use std::{env, io, thread};
use sysinfo::System;
use tokio::sync::{Mutex, OnceCell, RwLock};
use tokio::time::sleep;
use win_screenshot::prelude::find_window;
use windows::core::{w, PCSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowA, FindWindowW, SetForegroundWindow};
/*
Don't try refactor this piece of shit, it works on hopes, dreams and an incredibly poorly written web of functions
 */

#[derive(Debug)]
struct BotStats {
    start_time: DateTime<Utc>,
    players_kicked: i32,
}

static BOT_STATS: OnceLock<Arc<RwLock<BotStats>>> = OnceLock::new();

static CONFIG: OnceCell<Config> = OnceCell::const_new();
static CONFIG_FILENAME: OnceLock<String> = OnceLock::new();

static KICK_RECORD: OnceLock<Arc<Mutex<PlayerKickHistoryRecord>>> = OnceLock::new();

static mut DO_EXIT_ANNOUNCEMENT: bool = true;

async unsafe fn restart_bot() -> io::Result<()> {
    if let Some(record) = KICK_RECORD.get() {
        save_kick_record(record.lock().await.deref())?;
    }
    let current_exe = env::current_exe()?;
    Command::new(current_exe)
        .args([
            "0",
            "--config",
            CONFIG_FILENAME
                .get()
                .map(String::as_str)
                .unwrap_or("config.json"),
        ])
        .spawn()?;
    DO_EXIT_ANNOUNCEMENT = false;

    clear();

    exit(0);
}

fn bf1_running() -> bool {
    find_window("Battlefield™ 1").is_ok()
}

fn kill_bf1() {
    let s = System::new_all();
    if let Some(process) = s.processes_by_name("bf1".as_ref()).next() {
        process.kill();
    };
}

fn launch_bf1_join_server(path_str: String, game_id: String) -> io::Result<()> {
    Command::new(&path_str)
        .args([
            "-gameMode",
            "MP",
            "-role",
            "soldier",
            "-asSpectator",
            "true",
            "-gameId",
            &game_id,
        ])
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            failure::with_message(
                "Could not start Battlefield 1.\nCheck bf1_path and make sure the game is installed.",
                io::Error::new(
                error.kind(),
                format!("Failed to launch BF1 at {path_str}: {error}"),
                ),
            )
        })
}

async fn try_focus_bf1() {
    if let Ok(hwnd) = unsafe { FindWindowW(None, w!("Battlefield™ 1")) } {
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

async fn focus_bf1_once_running() {
    while !bf1_running() {
        sleep(Duration::from_secs(1)).await;
    }
    sleep(Duration::from_secs(10)).await;
    unsafe {
        if let Err(err) = restart_bot().await {
            log(&KickbotError::IOError(format!(
                "Failed to restart bot, please do it manually, {err}"
            )));
        }
    }

    let window_title = String::from("Battlefield™ 1");
    let mut hwnd: Option<HWND> = None;
    while hwnd.is_none() {
        hwnd = unsafe { FindWindowA(None, PCSTR::from_raw(window_title.as_ptr())).ok() };
        sleep(Duration::from_secs(1)).await;
    }

    if let Some(hwnd) = hwnd {
        if hwnd.0 != std::ptr::null_mut() {
            unsafe {
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }

    if let Ok(mut enigo) = Enigo::new(&Settings::default()) {
        let _ = enigo.button(Button::Left, Press);
        sleep(Duration::from_secs(1)).await;
        let _ = enigo.button(Button::Left, Release);
    }
}

unsafe extern "system" fn close_handler(_: u32) -> windows::core::BOOL {
    // Rust must never unwind across the Windows callback boundary.
    std::panic::catch_unwind(|| close_console()).unwrap_or(windows::core::BOOL(0))
}

unsafe fn close_console() -> windows::core::BOOL {
    if CONFIG.get().is_none() || BOT_STATS.get().is_none() || KICK_RECORD.get().is_none() {
        return windows::core::BOOL(0);
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            if (DO_EXIT_ANNOUNCEMENT) {
                announce_shutdown(
                    &CONFIG.get().unwrap().monitoring_webhook,
                    BOT_STATS.get().unwrap().read().await.deref(),
                )
                .await
                .expect("Something went wrong announcing shutdown");
            }

            let _ = save_kick_record(KICK_RECORD.get().unwrap().lock().await.deref()).inspect_err(
                |err| {
                    panic!(
                        "Something went wrong saving kick record, {}",
                        err.to_string()
                    )
                },
            );
        });

    windows::core::BOOL(1)
}

fn main() -> ExitCode {
    failure::finish(failure::run_guarded(run))
}

fn load_startup_config(filename: &str) -> io::Result<StartupConfig> {
    StartupConfig::load(filename).map_err(|error| {
        failure::with_message(
            "The configuration could not be loaded.\nCheck the selected file and the fields listed below.",
            io::Error::new(ErrorKind::InvalidData, error),
        )
    })
}

struct LaunchOptions {
    config_filename: String,
    should_announce_monitor: bool,
}

impl LaunchOptions {
    fn parse(args: impl IntoIterator<Item = String>) -> io::Result<Self> {
        let mut options = Self {
            config_filename: "config.json".to_string(),
            should_announce_monitor: true,
        };
        let mut args = args.into_iter();
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--config" => {
                    options.config_filename = args
                        .next()
                        .filter(|path| !path.is_empty() && !path.starts_with("--"))
                        .ok_or_else(|| {
                            io::Error::new(
                                ErrorKind::InvalidInput,
                                "Usage: vgkickbot.exe [0|1] [--config <path>]",
                            )
                        })?;
                }
                "0" => options.should_announce_monitor = false,
                "1" => options.should_announce_monitor = true,
                _ => {
                    return Err(io::Error::new(
                        ErrorKind::InvalidInput,
                        "Usage: vgkickbot.exe [0|1] [--config <path>]",
                    ));
                }
            }
        }
        Ok(options)
    }
}

#[cfg(test)]
mod launch_options_tests {
    use super::*;

    fn parse(args: &[&str]) -> io::Result<LaunchOptions> {
        LaunchOptions::parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn default_and_explicit_configuration_paths() {
        let defaults = parse(&[]).unwrap();
        assert_eq!(defaults.config_filename, "config.json");
        assert!(defaults.should_announce_monitor);
        let selected = parse(&["--config", "D:\\BF1 Bot\\settings.json"]).unwrap();
        assert_eq!(selected.config_filename, "D:\\BF1 Bot\\settings.json");
        assert!(selected.should_announce_monitor);
    }

    #[test]
    fn restart_announcement_argument_does_not_override_configuration() {
        for args in [
            ["0", "--config", "settings.json"],
            ["--config", "settings.json", "0"],
        ] {
            let options = parse(&args).unwrap();
            assert_eq!(options.config_filename, "settings.json");
            assert!(!options.should_announce_monitor);
        }
    }

    #[test]
    fn invalid_arguments_fail_without_panicking() {
        for args in [
            vec!["--config"],
            vec!["--config", ""],
            vec!["--config", "--unknown"],
            vec!["--validate-config"],
            vec!["--unknown"],
            vec!["unexpected"],
        ] {
            assert_eq!(parse(&args).err().unwrap().kind(), ErrorKind::InvalidInput);
        }
    }
}

fn run(panic_reports: &mut failure::PanicReceiver) -> io::Result<()> {
    let options = LaunchOptions::parse(env::args().skip(1)).map_err(|error| {
        failure::with_message("The launch arguments are invalid.\nUse --config followed by a configuration file path.", error)
    })?;
    let startup = load_startup_config(&options.config_filename)?;
    let _ = CONFIG_FILENAME.set(options.config_filename);

    unsafe {
        SetConsoleCtrlHandler(Some(close_handler), true)?;
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| failure::with_message("Could not initialize the bot runtime.", error))?
        .block_on(failure::supervise(
            main_thread(options.should_announce_monitor, startup),
            panic_reports,
        ))
}

async fn main_thread(should_announce_monitor: bool, mut startup: StartupConfig) -> io::Result<()> {
    let bf1_api = BF1Api::new(startup.sid(), startup.remid())
        .await
        .map_err(|error| {
            failure::with_message(
                "Could not authenticate with EA.\nCheck sid/remid and your network connection.",
                error,
            )
        })?;

    static BF1_API: OnceCell<BF1Api> = OnceCell::const_new();

    let display_names = bf1_api
        .get_display_names_by_persona_ids(vec![bf1_api.persona_id().as_str()])
        .await
        .map_err(|error| failure::with_message("Could not read your EA account details.", error))?;

    let user_name = display_names.first().cloned().ok_or_else(|| {
        failure::with_message(
            "Could not read your EA account details.",
            io::Error::other("EA returned no display name for the authenticated account"),
        )
    })?;
    let server = Arc::new(Mutex::new(bf1_api.get_server_by_name("![VG]").await.map_err(|error| {
        failure::with_message("Could not find or load the Battlefield 1 server.\nCheck the EA connection and server availability.", error)
    })?));

    let server_cached: ServerDetails = server.lock().await.clone();
    if !bf1_running() {
        launch_bf1_join_server(
            startup.bf1_path().to_string(),
            server_cached.game_id.clone(),
        )?;
        println!("Waiting for the Battlefield 1 window. Monitoring has not started.");
        while !bf1_running() {
            sleep(Duration::from_secs(1)).await;
        }
    }
    if startup.needs_calibration() {
        // No worker tasks, observer controls, model inference, or kick processing
        // exist until these six recognition fields have been supplied and saved.
        startup.calibrate_missing().map_err(|error| {
            failure::with_message(
                "Recognition setup did not complete.\nMonitoring has not started.",
                error,
            )
        })?;
    }
    let config = startup.into_config().await.map_err(|error| {
        failure::with_message("Could not initialize the bot settings or Discord webhooks.\nCheck the configured values.", error)
    })?;

    BOT_STATS
        .set(Arc::new(RwLock::new(BotStats {
            start_time: Utc::now(),
            players_kicked: 0,
        })))
        .unwrap();

    let bot_status = Arc::new(RwLock::new(BotStatus {
        status: StatusTypes::WaitingForBF1,
        timer_start: Instant::now(),
        map_start: String::new(),
        last_valid_name: None,
    }));

    if should_announce_monitor {
        announce_monitoring(
            &config.monitoring_webhook,
            BOT_STATS.get().unwrap().read().await.start_time,
        )
        .await
        .map_err(|error| failure::with_message("Could not send the monitoring-start notification.\nCheck monitoring_webhook and your network connection.", error))?;
    }

    let console = Arc::new(Mutex::new(console::Console::new(user_name)));

    let (width, height) = crossterm::terminal::size().map_err(|error| {
        failure::with_message(
            "Could not access the terminal.\nRun the bot in a console window.",
            error,
        )
    })?;
    console
        .lock()
        .await
        .update_static_area(
            server.lock().await.deref(),
            bot_status.read().await.deref(),
            BOT_STATS.get().unwrap().read().await.deref(),
            width,
            height,
        )
        .await;

    CONFIG.set(config).unwrap();
    BF1_API.set(bf1_api).unwrap();
    KICK_RECORD
        .set(Arc::new(Mutex::new(load_kick_history_record().map_err(|error| {
            failure::with_message("Could not load the kick history.\nCheck that the history file is readable and contains valid records.", error)
        })?)))
        .unwrap();

    let spec_cycle = Arc::new(Mutex::new(SpecCycle::new()));
    let game_state = Arc::new(RwLock::new(GameState::default()));
    let executors = Arc::new(Mutex::new(Executors::new(10)));

    let server_updated: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    let server_clone = server.clone();
    let bot_status_clone = bot_status.clone();
    let server_updated_clone = server_updated.clone();

    let server_clone_2 = server.clone();
    let bot_status_clone_2 = bot_status.clone();
    let console_clone = console.clone();

    tokio::spawn(async move {
        loop {
            // Yield so runtime shutdown can cancel this task before the Enter prompt.
            sleep(Duration::from_millis(50)).await;
            if !poll(Duration::ZERO).unwrap_or(false) {
                continue;
            }
            if let Ok(Event::Resize(width, height)) = read() {
                console_clone
                    .lock()
                    .await
                    .update_static_area(
                        server_clone_2.lock().await.deref(),
                        bot_status_clone_2.read().await.deref(),
                        BOT_STATS.get().unwrap().read().await.deref(),
                        width,
                        height,
                    )
                    .await;
            }
        }
    });

    let game_state_clone = game_state.clone();

    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(10)).await;

            let mut server_details = server_clone.lock().await;
            if let Err(err) = server_details.update_players(BF1_API.get().unwrap()).await {
                log(&err);
            }
            let gameid = Some(server_details.game_id.clone());
            if let Err(err) = server_details
                .update_server_details(BF1_API.get().unwrap(), gameid)
                .await
            {
                log(&err);
            }

            if server_details.player_count() < CONFIG.get().unwrap().min_players_for_kick as usize {
                bot_status_clone.write().await.status = StatusTypes::Disabled;
            } else {
                if bot_status_clone.read().await.status == StatusTypes::Disabled {
                    bot_status_clone.write().await.status = StatusTypes::WaitingForBF1;
                }
            }

            let (width, height) = crossterm::terminal::size().unwrap();
            console
                .lock()
                .await
                .update_static_area(
                    server_details.deref(),
                    bot_status_clone.read().await.deref(),
                    BOT_STATS.get().unwrap().read().await.deref(),
                    width,
                    height,
                )
                .await;

            let mut server_updated_writer = server_updated_clone.lock().await;
            *server_updated_writer = true;

            let game_state_read = game_state_clone.read().await;
            for (player, weapon) in game_state_read.pending_kick_players.iter() {
                kick_player(
                    BF1_API.get().unwrap(),
                    CONFIG.get().unwrap(),
                    KICK_RECORD.get().unwrap().clone(),
                    player,
                    weapon.name.clone(),
                    weapon.category.clone(),
                    game_state_clone.clone(),
                    server_details.deref(),
                    BOT_STATS.get().unwrap().clone(),
                    true,
                )
                .await
            }
        }
    });

    let classifier = Arc::new(Classifier::new());

    try_focus_bf1().await;

    // If we crash/don't have BF1, invalidate the last player name
    // So if we don't have a valid last player name then we know not to send a crash message if we don't read one
    loop {
        let do_cycle = async || {
            if let Ok(window) = active_win_pos_rs::get_active_window() {
                if window.title != "Battlefield™ 1" {
                    return false;
                }

                if matches!(
                    bot_status.read().await.status,
                    StatusTypes::Online
                        | StatusTypes::WaitingForNewMap
                        | StatusTypes::WaitingForBF1
                ) {
                    let mut is_updated = server_updated.lock().await;
                    if *is_updated {
                        let server_cached = server.lock().await.clone();
                        *is_updated = false;

                        let mut game_state = game_state.write().await;
                        let names: HashSet<String> = server_cached
                            .team1
                            .keys()
                            .chain(server_cached.team2.keys())
                            .cloned()
                            .collect();

                        game_state
                            .already_kicked_list_players
                            .retain(|name| names.contains(name))
                    }

                    if let Err(err) = execute(
                        BF1_API.get().unwrap(),
                        CONFIG.get().unwrap(),
                        KICK_RECORD.get().unwrap().clone(),
                        game_state.clone(),
                        executors.clone(),
                        spec_cycle.clone(),
                        server.clone(),
                        bot_status.clone(),
                        BOT_STATS.get().unwrap().clone(),
                        classifier.clone(),
                    )
                    .await
                    {
                        log(&err);
                    }
                }
                return true;
            }
            false
        };

        if !do_cycle().await {
            let bot_status_read = bot_status.read().await;

            if bot_status_read.status == StatusTypes::Crashed {
                drop(bot_status_read);
                if let Err(err) =
                    announce_bot_crashed(&CONFIG.get().unwrap().monitoring_webhook).await
                {
                    log(&err);
                }
                kill_bf1();
                while bf1_running() {
                    sleep(Duration::from_secs(1)).await;
                }
                launch_bf1_join_server(
                    CONFIG.get().unwrap().bf1_path.clone(),
                    server.lock().await.clone().game_id,
                )?;
                focus_bf1_once_running().await;

                bot_status.write().await.status = StatusTypes::WaitingForBF1;
                update_status(StatusTypes::WaitingForBF1);
            } else if bot_status_read.status != StatusTypes::WaitingForBF1
                && bot_status_read.status != StatusTypes::Disabled
            {
                drop(bot_status_read);
                bot_status.write().await.status = StatusTypes::WaitingForBF1;
                update_status(StatusTypes::WaitingForBF1);
            }
        }
    }
}
