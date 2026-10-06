use crate::console::log;
use crate::discord::{announce_player_multiple_kicks, DiscordWebhook};
use crate::errors::KickbotError;
use crate::errors::KickbotError::IOError;
use crate::recognition::enhance::RGB;
use crate::recognition::model::WeaponClasses;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use gestalt_ratio::gestalt_ratio;
use opencv::core::Rect;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::File;
use std::sync::OnceLock;
use std::time::Duration;

pub fn dates_to_csv_string(dates: &Vec<DateTime<Utc>>) -> Vec<String> {
    dates
        .iter()
        .map(|date| date.format("%Y-%m-%d %H:%M").to_string())
        .collect()
}

fn get_total_kicks(kick_record: &HashMap<String, Vec<DateTime<Utc>>>) -> u64 {
    kick_record
        .iter()
        .fold(0, |acc, (_, dates)| acc + dates.len() as u64)
}

pub fn weapon_kick_records_to_csv_strings(
    kick_records: &HashMap<String, Vec<DateTime<Utc>>>,
) -> Vec<String> {
    kick_records
        .iter()
        .flat_map(|(name, dates)| {
            let mut entries = vec![name.clone()];
            entries.extend(dates_to_csv_string(dates));
            entries
        })
        .collect()
}

#[derive(Debug)]
pub struct Vehicle {
    pub pretty_name: String,
    pub primary_names: Vec<String>,
    pub secondary_names: Vec<String>,
}

#[derive(Debug)]
pub struct Gadget {
    pretty_name: String,
    names: Vec<String>,
}

#[derive(Debug)]
pub struct Weapon {
    pub pretty_name: String,
    pub names: Vec<String>,
}

pub type PlayerKickHistoryRecord = HashMap<String, HashMap<String, Vec<DateTime<Utc>>>>;

static CSV_FILE_NAME: &str = "kick_history.csv";

fn get_csv_path() -> Result<&'static str, KickbotError> {
    if let Ok(false) = std::fs::exists(CSV_FILE_NAME) {
        log(&IOError(format!(
            "File {CSV_FILE_NAME} does not exist, creating"
        )));
        File::create_new(CSV_FILE_NAME)?;
    }
    Ok(CSV_FILE_NAME)
}

pub fn load_kick_history_record() -> Result<PlayerKickHistoryRecord, KickbotError> {
    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_path(get_csv_path()?)?;

    let mut player_kick_history_records = PlayerKickHistoryRecord::new();

    for result in csv_reader.records() {
        if let Ok(record) = result {
            let mut iter = record.iter();
            let player_name = match iter.next() {
                None => {
                    continue;
                }
                Some(player_name) => player_name,
            }
            .to_string();

            let mut weapon_records: HashMap<String, Vec<DateTime<Utc>>> = HashMap::new();

            let mut current_weapon = String::new();
            for entry in iter {
                if let Ok(date) = NaiveDateTime::parse_from_str(entry, "%Y-%m-%d %H:%M") {
                    if let Some(dates) = weapon_records.get_mut(&current_weapon) {
                        dates.push(DateTime::<Utc>::from_naive_utc_and_offset(date, Utc));
                    } else {
                        log(&KickbotError::IOError(
                            "Expected weapon name not date".to_string(),
                        ));
                        continue;
                    }
                } else {
                    current_weapon = entry.to_string();
                    weapon_records.insert(current_weapon.clone(), vec![]);
                }
            }
            player_kick_history_records.insert(player_name, weapon_records);
        }
    }

    Ok(player_kick_history_records)
}

pub async fn add_to_player_kick_record(
    kick_record: &mut PlayerKickHistoryRecord,
    config: &Config,
    player_name: String,
    weapon_type: WeaponClasses,
    kick_webhook: &Option<DiscordWebhook>,
    player_pid: &str,
) {
    let weapon_string = match weapon_type {
        WeaponClasses::AllowedPrimaryGuns => {
            return;
        }
        WeaponClasses::HeavyBomber | WeaponClasses::HMG | WeaponClasses::LMG => {
            match config.banned_vehicles.get(&weapon_type) {
                None => {
                    return;
                }
                Some(vehicle) => vehicle.pretty_name.clone(),
            }
        }
        WeaponClasses::SMG08 => "smg08".to_string(),
    };

    let date = Utc::now();

    match kick_record.entry(player_name.clone()) {
        Entry::Occupied(mut value) => {
            let mut records = value.get_mut();
            match records.entry(weapon_string.clone()) {
                Entry::Occupied(mut dates) => dates.get_mut().push(date),
                Entry::Vacant(_) => {
                    records.insert(weapon_string, vec![date]);
                }
            }
            let total_offences = get_total_kicks(value.get());
            if total_offences % config.kicks_to_ping == 0 {
                if let Err(err) = announce_player_multiple_kicks(
                    kick_webhook,
                    player_name.as_str(),
                    player_pid,
                    total_offences,
                    &value.get(),
                )
                .await
                {
                    log(&err);
                }
            };
        }
        Entry::Vacant(_) => {
            kick_record.insert(player_name, HashMap::from([(weapon_string, vec![date])]));
        }
    };
}

pub fn save_kick_record(kick_record: &PlayerKickHistoryRecord) -> Result<(), KickbotError> {
    let mut csv_writer = csv::Writer::from_path(get_csv_path()?)?;

    for (player_name, kick_record) in kick_record {
        let mut record = vec![player_name.clone()];
        let mut record_strings = weapon_kick_records_to_csv_strings(kick_record);

        record.append(&mut record_strings);

        csv_writer.write_record(&record)?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct Config {
    pub ea_cookies: EaCookies,
    pub bf1_path: String,
    pub kicks_to_ping: u64,
    pub min_players_for_kick: u64,
    pub kick_webhook: Option<DiscordWebhook>,
    pub monitoring_webhook: Option<DiscordWebhook>,
    pub player_similar_name_probability: f64,
    pub weapon_similar_name_probability: f64,
    pub save_screenshots: bool,
    pub rotate_delay: Duration,
    pub player_name_box: Rect,
    pub weapon_icon_probability: f32,
    pub weapon_icon_box: Rect,
    pub weapon_name_slot1_box: Rect,
    pub weapon_name_slot2_box: Rect,
    pub gadget_slot1_box: Option<Rect>,
    pub gadget_slot2_box: Option<Rect>,
    pub ally_colour: RGB,
    pub enemy_colour: RGB,
    pub banned_vehicles: HashMap<WeaponClasses, Vehicle>,
    pub banned_gadgets: Vec<Gadget>,
    pub banned_weapon: Weapon,
}

#[derive(Deserialize)]
pub struct EaCookies {
    pub sid: String,
    pub remid: String,
}

impl std::fmt::Debug for EaCookies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EaCookies")
            .field("sid", &"[REDACTED]")
            .field("remid", &"[REDACTED]")
            .finish()
    }
}

// Only validated data is deserialized into this type. Defaults are applied in
// memory; loading never rewrites the user's configuration file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(rename = "$schema", default)]
    _schema: Option<String>,
    sid: String,
    remid: String,
    bf1_path: String,
    kicks_to_ping: u64,
    min_players_for_kick: u64,
    kick_webhook: Option<String>,
    monitoring_webhook: Option<String>,
    player_similar_name_probability: f64,
    weapon_similar_name_probability: f64,
    #[serde(default)]
    save_screenshots: bool,
    rotate_delay: f64,
    #[serde(default)]
    player_name_box: Option<Rectangle>,
    weapon_icon_probability: f32,
    #[serde(default)]
    weapon_icon_box: Option<Rectangle>,
    #[serde(default)]
    weapon_slot_1_name_box: Option<Rectangle>,
    #[serde(default)]
    weapon_slot_2_name_box: Option<Rectangle>,
    #[serde(default)]
    gadget_slot_1_box: Option<Rectangle>,
    #[serde(default)]
    gadget_slot_2_box: Option<Rectangle>,
    #[serde(default)]
    ally_colour: Option<[u8; 3]>,
    #[serde(default)]
    enemy_colour: Option<[u8; 3]>,
    banned_vehicles: VehicleSettings,
    banned_weapon: WeaponSettings,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct Rectangle {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct CalibrationFields {
    pub player_name_box: Option<Rectangle>,
    pub weapon_icon_box: Option<Rectangle>,
    pub weapon_slot_1_name_box: Option<Rectangle>,
    pub weapon_slot_2_name_box: Option<Rectangle>,
    pub ally_colour: Option<[u8; 3]>,
    pub enemy_colour: Option<[u8; 3]>,
}

impl CalibrationFields {
    pub fn missing(&self) -> Vec<&'static str> {
        [
            ("player_name_box", self.player_name_box.is_none()),
            ("weapon_icon_box", self.weapon_icon_box.is_none()),
            (
                "weapon_slot_1_name_box",
                self.weapon_slot_1_name_box.is_none(),
            ),
            (
                "weapon_slot_2_name_box",
                self.weapon_slot_2_name_box.is_none(),
            ),
            ("ally_colour", self.ally_colour.is_none()),
            ("enemy_colour", self.enemy_colour.is_none()),
        ]
        .into_iter()
        .filter_map(|(name, absent)| absent.then_some(name))
        .collect()
    }
}

impl From<Rectangle> for Rect {
    fn from(value: Rectangle) -> Self {
        Rect::new(value.x, value.y, value.width, value.height)
    }
}

#[derive(Deserialize)]
struct VehicleNames {
    primary_names: Vec<String>,
    secondary_names: Vec<String>,
}

impl VehicleNames {
    fn into_vehicle(self, pretty_name: &str) -> Vehicle {
        Vehicle {
            pretty_name: pretty_name.to_string(),
            primary_names: self.primary_names,
            secondary_names: self.secondary_names,
        }
    }
}

#[derive(Deserialize)]
struct VehicleSettings {
    heavybomber: VehicleNames,
    hmg: VehicleNames,
}

#[derive(Deserialize)]
struct WeaponSettings {
    weapon_names: Vec<String>,
}

const CONFIG_SCHEMA: &str = include_str!("../config.schema.json");
static CONFIG_VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();

fn validate_json(json: &Value) -> Result<(), KickbotError> {
    let validator = CONFIG_VALIDATOR.get_or_init(|| {
        let schema: Value = serde_json::from_str(CONFIG_SCHEMA).map_err(|err| err.to_string())?;
        jsonschema::options()
            .should_validate_formats(true)
            .build(&schema)
            .map_err(|err| err.to_string())
    });
    let validator = validator.as_ref().map_err(|err| {
        KickbotError::JsonError(format!("Invalid embedded configuration schema: {err}"))
    })?;
    let errors: Vec<String> = validator
        .iter_errors(json)
        .map(|error| {
            let path = error.instance_path().to_string();
            let path = if path.is_empty() {
                "/".to_string()
            } else {
                path
            };
            // The library's normal Display includes instance values, including cookies
            // and webhook tokens. Mask every instance value in diagnostics instead.
            format!("  - {path}: {}", error.masked())
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(KickbotError::JsonError(format!(
            "Configuration validation failed:\n{}",
            errors.join("\n")
        )))
    }
}

impl FileConfig {
    fn from_json(mut json: Value) -> Result<Self, KickbotError> {
        let schema: Value = serde_json::from_str(CONFIG_SCHEMA)?;
        for (name, property) in schema["properties"]
            .as_object()
            .ok_or_else(|| KickbotError::JsonError("Invalid schema properties".to_string()))?
        {
            if let Some(default) = property.get("default").filter(|_| json.is_object()) {
                if let Some(existing) = json.get_mut(name) {
                    merge_defaults(default, existing);
                } else {
                    json.as_object_mut()
                        .unwrap()
                        .insert(name.clone(), default.clone());
                }
            }
        }
        validate_json(&json)?;
        normalize_integer_numbers(&mut json);
        serde_json::from_value(json).map_err(|_| {
            KickbotError::JsonError(
                "Validated configuration cannot be decoded into the application settings"
                    .to_string(),
            )
        })
    }

    fn load(filename: &str) -> Result<Self, KickbotError> {
        Self::from_json(read_json(filename)?)
    }

    fn calibration(&self) -> CalibrationFields {
        CalibrationFields {
            player_name_box: self.player_name_box,
            weapon_icon_box: self.weapon_icon_box,
            weapon_slot_1_name_box: self.weapon_slot_1_name_box,
            weapon_slot_2_name_box: self.weapon_slot_2_name_box,
            ally_colour: self.ally_colour,
            enemy_colour: self.enemy_colour,
        }
    }
}

fn read_json(filename: &str) -> Result<Value, KickbotError> {
    let reader = File::open(filename)
        .map_err(|err| KickbotError::JsonError(format!("Failed to open {filename}: {err}")))?;
    serde_json::from_reader(reader)
        .map_err(|err| KickbotError::JsonError(format!("Invalid JSON in {filename}: {err}")))
}

fn merge_defaults(default: &Value, value: &mut Value) {
    if let (Some(defaults), Some(fields)) = (default.as_object(), value.as_object_mut()) {
        for (key, default) in defaults {
            if let Some(existing) = fields.get_mut(key) {
                merge_defaults(default, existing);
            } else {
                fields.insert(key.clone(), default.clone());
            }
        }
    }
}

pub struct StartupConfig {
    settings: FileConfig,
    filename: String,
    original: Value,
}

impl StartupConfig {
    pub fn load(filename: &str) -> Result<Self, KickbotError> {
        let original = read_json(filename)?;
        let settings = FileConfig::from_json(original.clone())?;
        Ok(Self {
            settings,
            filename: filename.to_string(),
            original,
        })
    }
    pub fn sid(&self) -> &str {
        &self.settings.sid
    }
    pub fn remid(&self) -> &str {
        &self.settings.remid
    }
    pub fn bf1_path(&self) -> &str {
        &self.settings.bf1_path
    }
    pub fn needs_calibration(&self) -> bool {
        !self.settings.calibration().missing().is_empty()
    }

    pub fn calibrate_missing(&mut self) -> Result<(), KickbotError> {
        let missing = self.settings.calibration().missing();
        if missing.is_empty() {
            return Ok(());
        }
        println!("Waiting for recognition setup: {}. Monitoring and kicks are disabled until setup is complete.", missing.join(", "));
        let calibrated = crate::calibration::collect_missing(self.settings.calibration())?;
        self.save_calibration(calibrated)
    }

    fn save_calibration(&mut self, calibrated: CalibrationFields) -> Result<(), KickbotError> {
        if !calibrated.missing().is_empty() {
            return Err(IOError("Recognition setup is incomplete".to_string()));
        }
        let mut current = read_json(&self.filename)?;
        let values = serde_json::to_value(calibrated)?;
        for name in self.settings.calibration().missing() {
            if current.get(name) != self.original.get(name) {
                return Err(IOError(format!(
                    "Configuration changed during setup at {name}; no changes saved"
                )));
            }
            current
                .as_object_mut()
                .ok_or_else(|| IOError("Configuration must be an object".to_string()))?
                .insert(name.to_string(), values[name].clone());
        }
        let settings = FileConfig::from_json(current.clone())?;
        let path = std::fs::canonicalize(&self.filename)?;
        let temporary_dir = path
            .parent()
            .ok_or_else(|| IOError("Invalid configuration path".to_string()))?
            .join("target");
        std::fs::create_dir_all(&temporary_dir)?;
        let temporary = temporary_dir.join(format!(".calibration-{}.json", uuid::Uuid::new_v4()));
        let result = (|| -> Result<(), std::io::Error> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(&current).map_err(std::io::Error::other)?)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, &path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
        self.settings = settings;
        self.original = current;
        println!("Recognition setup saved to {}.", self.filename);
        Ok(())
    }

    pub async fn into_config(self) -> Result<Config, KickbotError> {
        Config::from_settings(self.settings).await
    }
}

// JSON Schema treats 10 and 10.0 as the same integer. Normalize that spelling
// after validation so Serde's integer fields accept both, without changing the file.
fn normalize_integer_numbers(value: &mut Value) {
    match value {
        Value::Number(number) if number.is_f64() => {
            if let Some(value_as_float) = number.as_f64() {
                if value_as_float >= 0.0
                    && value_as_float < 18446744073709551616.0
                    && value_as_float.fract() == 0.0
                {
                    *value = Value::from(value_as_float as u64);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(normalize_integer_numbers),
        Value::Object(fields) => fields.values_mut().for_each(normalize_integer_numbers),
        _ => {}
    }
}

fn rgb(value: [u8; 3]) -> RGB {
    RGB {
        r: i16::from(value[0]),
        g: i16::from(value[1]),
        b: i16::from(value[2]),
    }
}

impl Config {
    pub fn validate_file(filename: &str) -> Result<(), KickbotError> {
        FileConfig::load(filename).map(|_| ())
    }

    pub async fn read_config(filename: &str) -> Result<Config, KickbotError> {
        StartupConfig::load(filename)?.into_config().await
    }

    async fn from_settings(settings: FileConfig) -> Result<Config, KickbotError> {
        let fields = settings.calibration();
        let (
            Some(player_name_box),
            Some(weapon_icon_box),
            Some(weapon_slot_1_name_box),
            Some(weapon_slot_2_name_box),
            Some(ally_colour),
            Some(enemy_colour),
        ) = (
            fields.player_name_box,
            fields.weapon_icon_box,
            fields.weapon_slot_1_name_box,
            fields.weapon_slot_2_name_box,
            fields.ally_colour,
            fields.enemy_colour,
        )
        else {
            return Err(IOError(format!(
                "Recognition setup is required before monitoring: {}",
                fields.missing().join(", ")
            )));
        };
        let rotate_delay = Duration::try_from_secs_f64(settings.rotate_delay)
            .map_err(|_| KickbotError::JsonError("Invalid rotate_delay duration".to_string()))?;
        let kick_webhook = match settings.kick_webhook.filter(|url| !url.is_empty()) {
            Some(url) => Some(DiscordWebhook::new(&url, "SpecBot").await?),
            None => None,
        };
        let monitoring_webhook = match settings.monitoring_webhook.filter(|url| !url.is_empty()) {
            Some(url) => Some(DiscordWebhook::new(&url, "SpecBot").await?),
            None => None,
        };

        Ok(Config {
            ea_cookies: EaCookies {
                sid: settings.sid,
                remid: settings.remid,
            },
            bf1_path: settings.bf1_path,
            kicks_to_ping: settings.kicks_to_ping,
            min_players_for_kick: settings.min_players_for_kick,
            kick_webhook,
            monitoring_webhook,
            player_similar_name_probability: settings.player_similar_name_probability,
            weapon_similar_name_probability: settings.weapon_similar_name_probability,
            save_screenshots: settings.save_screenshots,
            rotate_delay,
            player_name_box: player_name_box.into(),
            weapon_icon_probability: settings.weapon_icon_probability,
            weapon_icon_box: weapon_icon_box.into(),
            weapon_name_slot1_box: weapon_slot_1_name_box.into(),
            weapon_name_slot2_box: weapon_slot_2_name_box.into(),
            gadget_slot1_box: settings.gadget_slot_1_box.map(Into::into),
            gadget_slot2_box: settings.gadget_slot_2_box.map(Into::into),
            ally_colour: rgb(ally_colour),
            enemy_colour: rgb(enemy_colour),
            banned_vehicles: HashMap::from([
                (
                    WeaponClasses::HeavyBomber,
                    settings
                        .banned_vehicles
                        .heavybomber
                        .into_vehicle("heavy bomber"),
                ),
                (
                    WeaponClasses::LMG,
                    settings.banned_vehicles.hmg.into_vehicle("mortar truck"),
                ),
            ]),
            banned_gadgets: vec![],
            banned_weapon: Weapon {
                pretty_name: "SMG08/18".to_string(),
                names: settings.banned_weapon.weapon_names,
            },
        })
    }

    pub fn are_similar(&self, string1: &str, string2: &str, probability: f64) -> bool {
        gestalt_ratio(string1, string2) >= probability
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Value {
        let mut value: Value =
            serde_json::from_str(include_str!("../config.example.json")).unwrap();
        let schema: Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        for (field, property) in schema["properties"].as_object().unwrap() {
            if let Some(default) = property.get("default") {
                value[field] = default.clone();
            }
        }
        for field in [
            "player_name_box",
            "weapon_icon_box",
            "weapon_slot_1_name_box",
            "weapon_slot_2_name_box",
        ] {
            value[field] = serde_json::json!({"x": 0, "y": 0, "width": 460, "height": 40});
        }
        value["ally_colour"] = serde_json::json!([64, 192, 255]);
        value["enemy_colour"] = serde_json::json!([255, 80, 64]);
        value
    }

    #[test]
    fn example_matches_schema_and_typed_loader() {
        let settings = FileConfig::from_json(example()).unwrap();
        assert_eq!(settings.kicks_to_ping, 10);
        assert_eq!(settings.sid, "<sid>");
    }

    #[test]
    fn only_credentials_and_game_path_are_required() {
        let schema: Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        assert_eq!(
            schema["required"],
            serde_json::json!(["sid", "remid", "bf1_path"])
        );
        let minimal =
            serde_json::json!({"sid":"test-sid", "remid":"test-remid", "bf1_path":"test-bf1.exe"});
        let settings = FileConfig::from_json(minimal).unwrap();
        assert_eq!(settings.kicks_to_ping, 10);
        assert_eq!(settings.min_players_for_kick, 10);
        assert_eq!(settings.player_similar_name_probability, 0.97);
        assert_eq!(settings.weapon_similar_name_probability, 0.85);
        assert_eq!(settings.rotate_delay, 2.0);
        assert_eq!(settings.weapon_icon_probability, 0.9);
        assert!(!settings.save_screenshots);
        assert!(settings.kick_webhook.is_none());
        assert!(settings.monitoring_webhook.is_none());
        assert_eq!(settings.calibration().missing().len(), 6);
        assert!(!settings.banned_weapon.weapon_names.is_empty());
    }

    #[test]
    fn explicit_values_override_defaults_and_partial_groups_keep_the_other_defaults() {
        let settings = FileConfig::from_json(serde_json::json!({
            "sid":"test-sid", "remid":"test-remid", "bf1_path":"test.exe",
            "kicks_to_ping": 25, "save_screenshots": true,
            "banned_vehicles": {"heavybomber": {"primary_names": ["custom"]}}
        }))
        .unwrap();
        assert_eq!(settings.kicks_to_ping, 25);
        assert!(settings.save_screenshots);
        assert_eq!(
            settings.banned_vehicles.heavybomber.primary_names,
            ["custom"]
        );
        assert!(!settings
            .banned_vehicles
            .heavybomber
            .secondary_names
            .is_empty());
        assert!(!settings.banned_vehicles.hmg.secondary_names.is_empty());
    }

    #[test]
    fn defaults_are_valid_and_invalid_root_documents_do_not_panic() {
        for invalid in [
            Value::Null,
            serde_json::json!([]),
            serde_json::json!("not an object"),
        ] {
            assert!(FileConfig::from_json(invalid).is_err());
        }
        let mut json =
            serde_json::json!({"sid":"test-sid", "remid":"test-remid", "bf1_path":"test.exe"});
        let schema: Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        for (field, property) in schema["properties"].as_object().unwrap() {
            if let Some(default) = property.get("default") {
                json[field] = default.clone();
            }
        }
        validate_json(&json).unwrap();
    }

    #[test]
    fn disabled_webhooks_and_complete_calibration_need_no_network_initialization() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for disabled in [Value::Null, serde_json::json!("")] {
            let mut json = example();
            json["kick_webhook"] = disabled.clone();
            json["monitoring_webhook"] = disabled;
            let settings = FileConfig::from_json(json).unwrap();
            let config = runtime.block_on(Config::from_settings(settings)).unwrap();
            assert!(config.kick_webhook.is_none());
            assert!(config.monitoring_webhook.is_none());
        }
    }

    #[test]
    fn incomplete_calibration_blocks_runtime_before_even_configured_webhooks() {
        let settings = FileConfig::from_json(serde_json::json!({
            "sid":"test-sid", "remid":"test-remid", "bf1_path":"test.exe",
            "kick_webhook":"https://example.invalid/test"
        }))
        .unwrap();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(Config::from_settings(settings));
        let error = result.unwrap_err().to_string();
        assert!(error.contains("Recognition setup"));
        assert!(error.contains("player_name_box"));
    }

    #[test]
    fn calibration_save_preserves_credentials_and_configured_values() {
        let directory = std::env::temp_dir().join(format!("bf1-setup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let filename = directory.join("config.json");
        let original = serde_json::json!({"sid":"private-test-sid", "remid":"private-test-remid", "bf1_path":"test.exe", "kicks_to_ping": 17, "ally_colour":[1,2,3]});
        std::fs::write(&filename, serde_json::to_vec(&original).unwrap()).unwrap();
        let mut startup = StartupConfig::load(filename.to_str().unwrap()).unwrap();
        assert!(startup.needs_calibration());
        let fields = FileConfig::from_json(example()).unwrap().calibration();
        startup.save_calibration(fields).unwrap();
        let saved = read_json(filename.to_str().unwrap()).unwrap();
        for key in ["sid", "remid", "bf1_path", "kicks_to_ping", "ally_colour"] {
            assert_eq!(saved[key], original[key]);
        }
        assert!(!startup.needs_calibration());
        assert!(saved.get("player_name_box").is_some());
        assert!(saved.get("min_players_for_kick").is_none());
        std::fs::remove_file(filename).unwrap();
        std::fs::remove_dir(directory.join("target")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn integral_float_notation_is_accepted_for_integer_settings() {
        let mut json = example();
        json["kicks_to_ping"] = serde_json::json!(10.0);
        json["player_name_box"]["width"] = serde_json::json!(460.0);
        json["ally_colour"][0] = serde_json::json!(64.0);
        let settings = FileConfig::from_json(json).unwrap();
        assert_eq!(settings.kicks_to_ping, 10);
        assert_eq!(settings.player_name_box.unwrap().width, 460);
        assert_eq!(settings.ally_colour.unwrap()[0], 64);
    }

    #[test]
    fn schema_preserves_unsigned_integer_limits() {
        let mut json = example();
        json["kicks_to_ping"] = serde_json::json!(u64::MAX);
        assert_eq!(
            FileConfig::from_json(json.clone()).unwrap().kicks_to_ping,
            u64::MAX
        );
        json["kicks_to_ping"] = serde_json::json!(18446744073709551616.0);
        assert!(validate_json(&json).is_err());
    }

    #[test]
    fn optional_settings_can_be_omitted() {
        let mut json = example();
        for field in [
            "$schema",
            "save_screenshots",
            "gadget_slot_1_box",
            "gadget_slot_2_box",
        ] {
            json.as_object_mut().unwrap().remove(field);
        }
        let settings = FileConfig::from_json(json).unwrap();
        assert!(!settings.save_screenshots);
        assert!(settings.gadget_slot_1_box.is_none());
        assert!(settings.gadget_slot_2_box.is_none());
    }

    #[test]
    fn invalid_present_optional_settings_are_rejected() {
        for (field, invalid) in [
            ("save_screenshots", serde_json::json!("false")),
            (
                "gadget_slot_1_box",
                serde_json::json!({"x": 0, "y": 0, "width": 0, "height": 10}),
            ),
            ("gadget_slot_2_box", Value::Null),
        ] {
            let mut json = example();
            json[field] = invalid;
            let message = validate_json(&json).unwrap_err().to_string();
            assert!(message.contains(field));
        }
    }

    #[test]
    fn every_required_field_is_checked_and_missing_fields_are_reported_together() {
        let schema: Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        for field in schema["required"].as_array().unwrap() {
            let field = field.as_str().unwrap();
            let mut json = example();
            json.as_object_mut().unwrap().remove(field);
            assert!(validate_json(&json)
                .unwrap_err()
                .to_string()
                .contains(field));
        }
        let message = validate_json(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(message.contains("sid"));
        assert!(message.contains("remid"));
        assert!(message.contains("bf1_path"));
    }

    #[test]
    fn invalid_ranges_nested_names_and_unknown_fields_are_rejected() {
        for pointer in [
            "/kicks_to_ping",
            "/player_similar_name_probability",
            "/ally_colour/0",
            "/player_name_box/width",
            "/banned_vehicles/heavybomber/primary_names/0",
            "/rotate_delay",
            "/kick_webhook",
        ] {
            let mut json = example();
            let invalid = match pointer {
                "/kicks_to_ping" | "/player_name_box/width" | "/rotate_delay" => {
                    serde_json::json!(0)
                }
                "/player_similar_name_probability" => serde_json::json!(1.1),
                "/ally_colour/0" => serde_json::json!(256),
                "/kick_webhook" => serde_json::json!("not-a-url"),
                _ => serde_json::json!(123),
            };
            *json.pointer_mut(pointer).unwrap() = invalid;
            assert!(validate_json(&json)
                .unwrap_err()
                .to_string()
                .contains(pointer));
        }
        let mut json = example();
        json["unexpected_setting"] = serde_json::json!(true);
        assert!(validate_json(&json).is_err());
    }

    #[test]
    fn schema_errors_never_print_credentials_or_the_entire_config() {
        let mut json = example();
        json["sid"] = serde_json::json!("SID_SECRET sentinel");
        json["remid"] = serde_json::json!("REMID_SECRET sentinel");
        json["kick_webhook"] = serde_json::json!("WEBHOOK_SECRET");
        json["monitoring_webhook"] = serde_json::json!("MONITORING_SECRET");
        let message = validate_json(&json).unwrap_err().to_string();
        for value in [
            "SID_SECRET",
            "REMID_SECRET",
            "WEBHOOK_SECRET",
            "MONITORING_SECRET",
        ] {
            assert!(!message.contains(value));
        }
        assert!(message.contains("/sid"));
        assert!(message.contains("/kick_webhook"));
    }

    #[test]
    fn invalid_configuration_stops_before_webhook_requests() {
        let path = std::env::temp_dir().join(format!("bf1-invalid-{}.json", uuid::Uuid::new_v4()));
        let mut json = example();
        json["kicks_to_ping"] = serde_json::json!(0);
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(Config::read_config(path.to_str().unwrap()));
        std::fs::remove_file(&path).unwrap();
        let message = result.unwrap_err().to_string();
        assert!(message.contains("/kicks_to_ping"));
        assert!(!message.contains("Discord"));
    }

    #[test]
    fn readme_documents_every_schema_field() {
        let schema: Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let readme = include_str!("../README.md");
        fn check_fields(schema: &Value, node: &Value, prefix: &str, readme: &str) {
            if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
                let resolved = schema
                    .pointer(reference.strip_prefix('#').unwrap())
                    .unwrap();
                // Rectangle fields are documented once in the shared rectangle table.
                let prefix = if reference == "#/$defs/rectangle" {
                    ""
                } else {
                    prefix
                };
                check_fields(schema, resolved, prefix, readme);
            }
            if let Some(properties) = node.get("properties").and_then(Value::as_object) {
                for (name, property) in properties {
                    let field = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    assert!(
                        readme.contains(&format!("| \x60{field}\x60 |")),
                        "Missing README field: {field}"
                    );
                    check_fields(schema, property, &field, readme);
                }
            }
        }
        check_fields(&schema, &schema, "", readme);
    }

    #[test]
    fn cookie_debug_output_redacts_credentials() {
        let cookies = EaCookies {
            sid: "test-sid-cookie-value".to_string(),
            remid: "test-remid-cookie-value".to_string(),
        };
        let output = format!("{cookies:?}");
        assert!(!output.contains(&cookies.sid));
        assert!(!output.contains(&cookies.remid));
        assert!(output.contains("[REDACTED]"));
    }
}
