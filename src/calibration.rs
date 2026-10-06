use crate::config::{CalibrationFields, Rectangle};
use crate::errors::KickbotError;
use crate::recognition::screenshot::Screenshot;
use opencv::core::{Mat, MatTraitConst, Point, Scalar, Vec3b};
use opencv::{highgui, imgproc};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

struct SelectionWindow(String);

impl SelectionWindow {
    fn open(title: &str) -> Result<Self, KickbotError> {
        highgui::named_window(title, highgui::WINDOW_NORMAL)?;
        let window = Self(title.to_string());
        highgui::resize_window(title, 1280, 720)?;
        Ok(window)
    }
}

impl Drop for SelectionWindow {
    fn drop(&mut self) {
        let _ = highgui::destroy_window(&self.0);
    }
}

fn prompt(message: &str) -> Result<String, KickbotError> {
    print!("{message}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Err(KickbotError::IOError(
            "Input closed; recognition setup canceled".to_string(),
        ));
    }
    let line = line.trim().to_string();
    if line.eq_ignore_ascii_case("q") {
        return Err(KickbotError::IOError(
            "Recognition setup canceled; monitoring has not started".to_string(),
        ));
    }
    Ok(line)
}

fn capture() -> Result<Mat, KickbotError> {
    let screenshot = Screenshot::take_screenshot()?;
    rgba_to_bgr(&screenshot.image)
}

fn rgba_to_bgr(image: &Mat) -> Result<Mat, KickbotError> {
    // win-screenshot returns RGBA pixels for window captures.
    let mut bgr = Mat::default();
    imgproc::cvt_color_def(image, &mut bgr, imgproc::COLOR_RGBA2BGR)?;
    Ok(bgr)
}

fn parse_rectangle(line: &str) -> Option<Rectangle> {
    let values: Vec<i32> = line
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let [x, y, width, height] = values.as_slice() else {
        return None;
    };
    let rectangle = Rectangle {
        x: *x,
        y: *y,
        width: *width,
        height: *height,
    };
    (rectangle.x >= 0
        && rectangle.y >= 0
        && rectangle.width > 0
        && rectangle.height > 0
        && values.iter().all(|value| *value <= 1073741823))
    .then_some(rectangle)
}

fn fits(rectangle: Rectangle, width: i32, height: i32) -> bool {
    rectangle.x >= 0
        && rectangle.y >= 0
        && rectangle.width > 0
        && rectangle.height > 0
        && rectangle
            .x
            .checked_add(rectangle.width)
            .is_some_and(|edge| edge <= width)
        && rectangle
            .y
            .checked_add(rectangle.height)
            .is_some_and(|edge| edge <= height)
}

#[derive(Clone, Copy, Default)]
struct DragSelection {
    anchor: Option<Point>,
    end: Option<Point>,
    dragging: bool,
}

impl DragSelection {
    fn rectangle(self) -> Option<Rectangle> {
        let (Some(anchor), Some(end)) = (self.anchor, self.end) else {
            return None;
        };
        let rectangle = Rectangle {
            x: anchor.x.min(end.x),
            y: anchor.y.min(end.y),
            width: (anchor.x - end.x).abs(),
            height: (anchor.y - end.y).abs(),
        };
        (rectangle.width > 0 && rectangle.height > 0).then_some(rectangle)
    }
}

fn select_rectangle(frame: &Mat, title: &str) -> Result<Option<Rectangle>, KickbotError> {
    let _window = SelectionWindow::open(title)?;
    let selection = Arc::new(Mutex::new(DragSelection::default()));
    let callback_selection = Arc::clone(&selection);
    let (width, height) = (frame.cols(), frame.rows());
    highgui::set_mouse_callback(
        title,
        Some(Box::new(move |event, x, y, _| {
            if let Ok(mut selection) = callback_selection.lock() {
                let point = Point::new(x.clamp(0, width), y.clamp(0, height));
                if event == highgui::EVENT_LBUTTONDOWN {
                    *selection = DragSelection {
                        anchor: Some(point),
                        end: Some(point),
                        dragging: true,
                    };
                } else if selection.dragging
                    && (event == highgui::EVENT_MOUSEMOVE || event == highgui::EVENT_LBUTTONUP)
                {
                    selection.end = Some(point);
                    if event == highgui::EVENT_LBUTTONUP {
                        selection.dragging = false;
                    }
                }
            }
        })),
    )?;
    highgui::imshow(title, frame)?;
    let mut last_drawn = None;
    loop {
        let key = highgui::wait_key(30)?;
        if key == 27
            || highgui::get_window_property(title, highgui::WND_PROP_VISIBLE).unwrap_or(0.0) < 1.0
        {
            return Ok(None);
        }
        if key == i32::from(b'c') || key == i32::from(b'C') {
            *selection
                .lock()
                .map_err(|_| KickbotError::IOError("Region picker lock failed".to_string()))? =
                DragSelection::default();
        }
        let rectangle = selection
            .lock()
            .map_err(|_| KickbotError::IOError("Region picker lock failed".to_string()))?
            .rectangle();
        if key == 13 || key == 32 {
            return Ok(rectangle);
        }
        if rectangle != last_drawn {
            let mut displayed = frame.try_clone()?;
            if let Some(rectangle) = rectangle {
                imgproc::rectangle(
                    &mut displayed,
                    rectangle.into(),
                    Scalar::new(0.0, 255.0, 0.0, 0.0),
                    2,
                    imgproc::LINE_8,
                    0,
                )?;
            }
            highgui::imshow(title, &displayed)?;
            last_drawn = rectangle;
        }
    }
}

fn choose_rectangle(field: &str, label: &str) -> Result<Rectangle, KickbotError> {
    loop {
        println!("\n{field}: {label}. Make the spectator HUD visible; keep BF1 restored in windowed/borderless mode.");
        let line = prompt(
            "Enter x y width height, or press Enter to select on a fresh screenshot (q quits): ",
        )?;
        if !line.is_empty() {
            if let Some(rectangle) = parse_rectangle(&line) {
                if let Ok(frame) = capture() {
                    if !fits(rectangle, frame.cols(), frame.rows()) {
                        println!("That rectangle does not fit the captured game image.");
                        continue;
                    }
                }
                return Ok(rectangle);
            }
            println!("Use four integers: non-negative x/y and positive width/height.");
            continue;
        }
        let frame = match capture() {
            Ok(frame) => frame,
            Err(_) => {
                println!("Could not capture BF1. Restore the game window and try again, or enter coordinates manually.");
                continue;
            }
        };
        let title = format!("Setup: {label}");
        println!("Drag a rectangle. Enter/Space confirms, C resets, Escape or closing the window cancels this selection.");
        if let Some(rectangle) = select_rectangle(&frame, &title)?
            .filter(|rectangle| fits(*rectangle, frame.cols(), frame.rows()))
        {
            println!(
                "Selected: {} {} {} {}.",
                rectangle.x, rectangle.y, rectangle.width, rectangle.height
            );
            return Ok(rectangle);
        }
        println!("No valid region selected; still waiting for this field.");
    }
}

fn parse_colour(line: &str) -> Option<[u8; 3]> {
    let values: Vec<u8> = line
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    values.try_into().ok()
}

fn rgb_at(frame: &Mat, point: Point) -> Result<Option<[u8; 3]>, KickbotError> {
    if point.x < 0 || point.y < 0 || point.x >= frame.cols() || point.y >= frame.rows() {
        return Ok(None);
    }
    let pixel = frame.at_2d::<Vec3b>(point.y, point.x)?;
    Ok(Some([pixel[2], pixel[1], pixel[0]]))
}

fn pick_colour(frame: &Mat, label: &str) -> Result<Option<[u8; 3]>, KickbotError> {
    let title = format!("Setup: {label} (click text, Esc cancels)");
    let window = SelectionWindow::open(&title)?;
    let point = Arc::new(Mutex::new(None));
    let callback_point = Arc::clone(&point);
    highgui::set_mouse_callback(
        &title,
        Some(Box::new(move |event, x, y, _| {
            if event == highgui::EVENT_LBUTTONDOWN {
                if let Ok(mut point) = callback_point.lock() {
                    *point = Some(Point::new(x, y));
                }
            }
        })),
    )?;
    highgui::imshow(&title, frame)?;
    loop {
        let key = highgui::wait_key(30)?;
        let selected = *point
            .lock()
            .map_err(|_| KickbotError::IOError("Colour picker lock failed".to_string()))?;
        if let Some(selected) = selected {
            let result = rgb_at(frame, selected)?;
            drop(window);
            return Ok(result);
        }
        if key == 27
            || highgui::get_window_property(&title, highgui::WND_PROP_VISIBLE).unwrap_or(0.0) < 1.0
        {
            return Ok(None);
        }
    }
}

fn choose_colour(field: &str, label: &str) -> Result<[u8; 3], KickbotError> {
    loop {
        println!("\n{field}: {label}. Show the corresponding allied/enemy player-name text before opening the picker.");
        let line = prompt("Enter R G B (0-255), or press Enter to click the text on a fresh screenshot (q quits): ")?;
        if !line.is_empty() {
            if let Some(colour) = parse_colour(&line) {
                return Ok(colour);
            }
            println!("Use three integer RGB components between 0 and 255.");
            continue;
        }
        let frame = match capture() {
            Ok(frame) => frame,
            Err(_) => {
                println!("Could not capture BF1. Restore the game window and try again, or enter RGB manually.");
                continue;
            }
        };
        if let Some(colour) = pick_colour(&frame, label)? {
            println!("Selected RGB: {} {} {}.", colour[0], colour[1], colour[2]);
            if prompt("Press Enter to use this colour, or type r to retry: ")?.is_empty() {
                return Ok(colour);
            }
        } else {
            println!("Selection canceled; still waiting for this colour.");
        }
    }
}

pub(crate) fn collect_missing(
    mut fields: CalibrationFields,
) -> Result<CalibrationFields, KickbotError> {
    if fields.missing().is_empty() {
        return Ok(fields);
    }
    println!("Set up the real spectator HUD. Selections use original captured-image pixels. No monitoring or kicks run during setup.");
    for (name, label, target) in [
        (
            "player_name_box",
            "player name",
            &mut fields.player_name_box,
        ),
        (
            "weapon_icon_box",
            "weapon or vehicle icon",
            &mut fields.weapon_icon_box,
        ),
        (
            "weapon_slot_1_name_box",
            "first weapon-slot name",
            &mut fields.weapon_slot_1_name_box,
        ),
        (
            "weapon_slot_2_name_box",
            "second weapon-slot name",
            &mut fields.weapon_slot_2_name_box,
        ),
    ] {
        if target.is_none() {
            *target = Some(choose_rectangle(name, label)?);
        }
    }
    if fields.ally_colour.is_none() {
        fields.ally_colour = Some(choose_colour("ally_colour", "allied player-name text")?);
    }
    if fields.enemy_colour.is_none() {
        fields.enemy_colour = Some(choose_colour("enemy_colour", "enemy player-name text")?);
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::core::{CV_8UC3, CV_8UC4};

    #[test]
    fn dragged_rectangles_support_both_directions_and_ignore_empty_selections() {
        let selection = DragSelection {
            anchor: Some(Point::new(40, 60)),
            end: Some(Point::new(10, 20)),
            dragging: false,
        };
        assert_eq!(
            selection.rectangle(),
            Some(Rectangle {
                x: 10,
                y: 20,
                width: 30,
                height: 40
            })
        );
        assert!(DragSelection::default().rectangle().is_none());
        assert!(DragSelection {
            anchor: Some(Point::new(10, 20)),
            end: Some(Point::new(10, 20)),
            dragging: false
        }
        .rectangle()
        .is_none());
    }

    #[test]
    fn manual_input_rejects_invalid_regions_and_colours() {
        assert!(parse_rectangle("1 2 30 40").is_some());
        for input in ["-1 0 10 10", "0 0 0 10", "0 0 1", "x y width height"] {
            assert!(parse_rectangle(input).is_none());
        }
        assert_eq!(parse_colour("64 192 255"), Some([64, 192, 255]));
        for input in ["-1 2 3", "1 2 256", "1 2", "rgb"] {
            assert!(parse_colour(input).is_none());
        }
    }

    #[test]
    fn selected_regions_must_fit_the_original_capture() {
        assert!(fits(
            Rectangle {
                x: 10,
                y: 20,
                width: 30,
                height: 40
            },
            100,
            100
        ));
        assert!(!fits(
            Rectangle {
                x: 90,
                y: 20,
                width: 30,
                height: 40
            },
            100,
            100
        ));
        assert!(!fits(
            Rectangle {
                x: i32::MAX,
                y: 0,
                width: 10,
                height: 10
            },
            100,
            100
        ));
    }

    #[test]
    fn colour_picker_returns_rgb_from_bgr_pixels() {
        let frame =
            Mat::new_rows_cols_with_default(2, 2, CV_8UC3, Scalar::new(10.0, 20.0, 30.0, 0.0))
                .unwrap();
        assert_eq!(
            rgb_at(&frame, Point::new(1, 1)).unwrap(),
            Some([30, 20, 10])
        );
        assert_eq!(rgb_at(&frame, Point::new(2, 1)).unwrap(), None);
    }

    #[test]
    fn screenshot_conversion_preserves_red_and_blue_channels() {
        let rgba =
            Mat::new_rows_cols_with_default(1, 1, CV_8UC4, Scalar::new(240.0, 100.0, 20.0, 255.0))
                .unwrap();
        let bgr = rgba_to_bgr(&rgba).unwrap();
        assert_eq!(
            rgb_at(&bgr, Point::new(0, 0)).unwrap(),
            Some([240, 100, 20])
        );
    }
}
