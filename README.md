# Battlefield 1 Kickbot

## Building on Windows

For a fresh Windows 10/11 x64 installation:

1. Install [Visual Studio Build Tools](https://learn.microsoft.com/en-us/cpp/overview/acquire-msvc?view=msvc-170).
   Select **Desktop development with C++**
   and ensure **MSVC C++ x64/x86 build tools** and a **Windows 10 or Windows 11 SDK**
   are selected. The full Visual Studio IDE is not required.
2. Install [Rust using the x64 rustup installer](https://rust-lang.org/tools/install/?platform_override=win).
   Use the standard **stable MSVC** toolchain, `x86_64-pc-windows-msvc`.
   The project requires **Rust 1.95 or newer**; Cargo is installed with Rust.
3. Open a **new 64-bit PowerShell window** after installation so the updated PATH
   takes effect, change to the repository root, and run:

```powershell
.\build.ps1
# For a release build:
.\build.ps1 -Release
```

If PowerShell blocks script execution, run it with an execution policy override
limited to the new process:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\build.ps1
# For a release build:
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\build.ps1 -Release
```

The first build needs an internet connection. The script prepares cargo-vcpkg,
Tesseract, Leptonica, OpenCV, Clang/libclang, and other build dependencies as
needed, then builds the application. Installing these packages separately is not
required. Downloaded tools and native libraries live under the Git-ignored
`target` directory. Rust and native library dependencies are declared in `Cargo.toml`.
The output is `target/debug/vgkickbot.exe`, or `target/release/vgkickbot.exe`
when using `-Release`. `config.json` and an installed Battlefield 1
are needed to run the bot, not to compile it.

The script first looks for the required versions in the system, reports found
components with `[found]`, and uses them without downloading. It searches PATH,
`VCPKG_ROOT`, `CLANG_PATH`, `LIBCLANG_PATH`, standard installation directories,
and tools already prepared under `target`. Incompatible or incomplete components
are reported with `[skip]`. Versions are declared in `Cargo.toml`: cargo-vcpkg
0.1.7, LLVM 23.1.2, Tesseract 5.5.3, Leptonica 1.87.0, and OpenCV 4.14.0.

Native packages can be reused from an existing vcpkg installation if their
versions, the `x64-windows-static-md` triplet, and required OpenCV features match.
When the complete package set is already present, cargo-vcpkg is not needed and
the script skips fetching vcpkg and preparing native packages. Otherwise it uses
an installed cargo-vcpkg 0.1.7 or installs it in `target/build-tools/cargo`, then
runs `cargo vcpkg build` for the local installation. Existing local packages and
the auxiliary CMake/Ninja tools are reused by vcpkg.

The OpenCV binding generator also needs `clang.exe` to discover C++ include
paths. The script separately checks Clang with builtin headers and a loadable
`libclang.dll` of the required version. If either is missing, it downloads the
official LLVM archive (about 264 MiB), verifies its SHA-256, and extracts only
the missing components into `target/build-tools/llvm`. A verified cached archive
is reused. The DLL, Clang, headers, and LLVM license are all excluded from Git.
vcpkg supplies an archive extractor when needed.
Initial extraction temporarily needs about 4 GiB for an intermediate tar; the
script removes that tar after successful preparation.

`cargo vcpkg build` downloads the pinned vcpkg revision into `target/vcpkg` and
builds Tesseract, Leptonica, and the required OpenCV modules. The project Cargo
configuration selects `x64-windows-static-md`, which statically links these
libraries while retaining the normal dynamic Windows C runtime.

The script sets selected tool paths for the build and restores the terminal's
settings afterwards. `.cargo/config.toml` provides local defaults. Use `build.ps1` for automatic discovery on
subsequent builds too; plain `cargo build` uses the local defaults or paths
already set in your terminal. Use `-Check` to check compilation without linking.
The script does not install global dependencies. Rust's crate cache remains in
the usual user directory.

## Configuration

Copy `config.example.json` to `config.json` and fill in the local settings:

```powershell
Copy-Item -LiteralPath config.example.json -Destination config.json
```

All application settings are read from the selected JSON configuration file.
By default, the bot uses `config.json` in the current working directory. Replace the example's
cookie and executable-path placeholders with real values. Paste EA
cookies without the `sid=` or `remid=` prefix. Use an EA account that can administer
the intended server. `config.json` is excluded from Git because it contains
credentials; commit only the example with placeholder values.

At startup the bot validates the complete document against
[`config.schema.json`](config.schema.json), using JSON Schema Draft 2020-12,
before initializing webhooks, contacting EA, or starting monitoring. Invalid
configuration stops startup and lists all schema violations with JSON Pointer
paths such as `/player_name_box/width`. Instance values are masked in these
messages so cookie values and webhook tokens are not printed.

Any fatal application error prints a diagnostic and waits for Enter before
closing, including configuration, command-line, EA authentication, game launch,
and model-loading errors. Rust panics in the main thread or background tasks
also stop monitoring and use the same prompt; panic diagnostics report the source
location without dumping potentially sensitive values. The process exits with a
nonzero status; if standard input is closed, it exits without waiting.
Configuration is checked before the runtime, console handler, or network requests
are initialized. Schema validation checks formats, not whether cookies are valid
EA credentials: a non-empty value such as `test` may pass the schema and then fail
authentication, which also leaves the error visible until Enter.

Fatal diagnostics show a highlighted error heading and a plain-language
explanation, a blank line, the original technical error, and another blank line
before `Press Enter to close...`. Colours are used only in a terminal; redirected
output remains plain text. Error handlers can supply a separate explanatory
message with `failure::with_message` while retaining the original error details.

Required fields must be present. Optional fields may be omitted, but must match
their schema when present. Only webhook fields accept `null` or an empty string
to explicitly disable them.
Unknown fields are rejected, including unknown nested fields. Defaults are
applied from the schema in memory and do not rewrite the configuration file.
Interactive recognition setup saves only newly supplied regions and colours. The schema is
embedded in the executable; distributing the JSON schema file is optional for
running the bot. The optional `$schema` field helps editors validate a document
and does not replace the embedded runtime schema.

Start the bot with the default configuration, or select another file with
`--config <path>`:

```powershell
.\target\debug\vgkickbot.exe
.\target\debug\vgkickbot.exe --config "D:\BF1 Bot\settings.json"
# When running through Cargo:
cargo run -- --config "D:\BF1 Bot\settings.json"
```

Relative configuration paths are resolved from the current working directory.
`--config` starts the normal bot with that file; validation is always part of
startup. The selected path is also retained during automatic bot restarts.
The optional positional argument `0` suppresses the monitoring-start announcement
(`1` enables it, which is the default); it can be combined with `--config`.

### First-run recognition setup

Only `sid`, `remid`, and `bf1_path` are required in the file. The shipped example is
minimal so its coordinates cannot accidentally be mistaken for your actual HUD.

Startup proceeds in this order:

1. Validate the supplied configuration and apply ordinary defaults.
2. Authenticate with EA, locate the server, and launch BF1 as an observer if
   the game is not already running. Launching does not block until the game exits.
3. Wait for the game window. If any of the six recognition fields below are
   missing, pause for setup while the game remains running.
4. Save completed setup, initialize any configured webhooks, and start the
   monitoring workers, recognition models, and kick processing.

The six setup fields are `player_name_box`, `weapon_icon_box`,
`weapon_slot_1_name_box`, `weapon_slot_2_name_box`, `ally_colour`, and
`enemy_colour`. There are no guessed defaults for these values.

Join the correct server as a spectator and show the HUD. Keep BF1 restored in
windowed or borderless mode; a minimized/exclusive-fullscreen game may not
provide a usable capture. For each missing rectangle, enter
`x y width height` in the terminal or press Enter to open a fresh screenshot.
Drag the region and confirm with Enter/Space; C resets the selection. Escape or
closing the picker cancels that selection and returns to the prompt. For each missing colour,
enter `R G B` or press Enter and click the appropriate allied/enemy name text on
a fresh screenshot. Confirm the sampled RGB value in the terminal, or retry.
The picker uses original captured-image coordinates even when its window is resized.

Already configured fields are retained and are not requested again. Once all
six values are supplied, they are validated and saved together in
the selected configuration file; credentials, explicit settings, and unrelated edits are preserved.
Monitoring and kicks remain disabled until setup and saving have succeeded.
Closing input or typing `q` cancels startup; the game is left running. A canceled
mouse selection keeps the bot waiting for that field. If a calibration field
was changed in the file while setup was open, saving stops rather than
overwriting it. To recalibrate a region or colour later, remove that field from
the file and restart the bot.

### Field reference

Every top-level configuration field is listed below. Probabilities are numbers
in the inclusive range `0` to `1`. Name matching uses string similarity, rather
than calibrated OCR confidence.

| Field | Required / default | Type and purpose |
| --- | --- | --- |
| `$schema` | Optional; omitted | Non-empty string referencing an editor schema, normally `./config.schema.json`. |
| `sid` | Required | EA `sid` cookie value. Non-empty; whitespace and semicolons are not allowed. |
| `remid` | Required | EA `remid` cookie value. Non-empty; whitespace and semicolons are not allowed. |
| `bf1_path` | Required | Non-empty path to `bf1.exe`, used to launch Battlefield 1 and join the server as an observer. |
| `kick_webhook` | Optional; disabled | Absolute HTTPS Discord webhook URL for kick success/failure and repeat-offender notifications. Omitted, `null`, or `""` means no initialization or notification requests. |
| `monitoring_webhook` | Optional; disabled | Absolute HTTPS Discord webhook URL for monitoring start, stop, and restart notifications. The same URL may be used for both webhook fields. Omitted, `null`, or `""` disables all monitoring notifications. |
| `kicks_to_ping` | Optional; `10` | Positive integer: send a repeat-offender notification after every N accumulated kicks for a player. Zero is invalid. |
| `min_players_for_kick` | Optional; `10` | Non-negative integer: minimum server population used to enable kicking. The existing implementation checks this periodically; it is not an immediate cancellation of pending kicks. |
| `player_similar_name_probability` | Optional; `0.97` | Probability: similarity threshold for player-name matching and detecting repeated observations of the same player. |
| `weapon_similar_name_probability` | Optional; `0.85` | Probability: similarity threshold for matching OCR weapon/vehicle names against configured aliases. |
| `save_screenshots` | Optional; `false` | Boolean: retain and save screenshots when a violation is detected. Create the `screenshots` directory before enabling it. |
| `rotate_delay` | Optional; `2.0` | Number of seconds before detection and observer rotation; greater than zero and at most `86400`. |
| `player_name_box` | Optional; requested at startup | Rectangle containing the observed player's name for OCR. |
| `weapon_icon_probability` | Optional; `0.9` | Probability: classifier confidence threshold for selecting an icon-based category rather than the OCR fallback. The existing classifier confidence calculation has known limitations. |
| `weapon_icon_box` | Optional; requested at startup | Rectangle containing the weapon/vehicle icon for the ONNX classifier. |
| `weapon_slot_1_name_box` | Optional; requested at startup | Rectangle containing the first weapon-slot name for OCR. |
| `weapon_slot_2_name_box` | Optional; requested at startup | Rectangle containing the second weapon-slot name for OCR and vehicle confirmation. |
| `gadget_slot_1_box` | Optional; omitted | Legacy rectangle for the first gadget slot. Gadget detection is currently unimplemented; this field has no active effect. |
| `gadget_slot_2_box` | Optional; omitted | Legacy rectangle for the second gadget slot. Gadget detection is currently unimplemented; this field has no active effect. |
| `ally_colour` | Optional; requested at startup | Exactly three integer RGB components `[R, G, B]`, each `0`–`255`, for allied player-name text filtering. |
| `enemy_colour` | Optional; requested at startup | Exactly three integer RGB components `[R, G, B]`, each `0`–`255`, for enemy player-name text filtering. |
| `banned_vehicles` | Optional; defaults below | Object containing the `heavybomber` and legacy `hmg` groups described below. Missing groups or alias arrays receive their defaults. |
| `banned_weapon` | Optional; defaults below | Object containing the `weapon_names` array described below. Missing values receive their defaults. |

Use JSON escaping for Windows paths:

```json
"bf1_path": "C:\\Program Files\\EA Games\\Battlefield 1\\bf1.exe"
```

All rectangle objects have these required fields:

| Field | Type and purpose |
| --- | --- |
| `x` | Non-negative integer: horizontal offset from the captured image's left edge. |
| `y` | Non-negative integer: vertical offset from the captured image's top edge. |
| `width` | Positive integer: region width in pixels. |
| `height` | Positive integer: region height in pixels. |

Rectangle components are limited to `1073741823` to keep coordinate-plus-size
arithmetic within signed 32-bit limits. Each region must also fit the actual
captured image; startup validation cannot know its dimensions. Measure regions
for the real resolution, window borders, HUD scale, and game language. These
values are requested interactively rather than taken from example coordinates.

Nested prohibited-item fields:

| Field | Default when omitted | Purpose |
| --- | --- | --- |
| `banned_vehicles.heavybomber` | Alias arrays below | Heavy-bomber OCR confirmation group. |
| `banned_vehicles.heavybomber.primary_names` | `["HEAVY BOMBER", "BOMBS"]` | Array of OCR aliases checked against the first weapon slot. |
| `banned_vehicles.heavybomber.secondary_names` | `["BOMBS", "HEAVY BOMBER"]` | Array of OCR aliases checked against the second weapon slot. |
| `banned_vehicles.hmg` | Alias arrays below | Legacy JSON key mapped to the mortar-truck detector and the internal `LMG` category. Keep this exact key. |
| `banned_vehicles.hmg.primary_names` | `["LMG"]` | Legacy array of OCR aliases, loaded but currently unused by the mortar-truck branch. |
| `banned_vehicles.hmg.secondary_names` | `["MORTAR", "MORTAR TRUCK"]` | Array of mortar-truck OCR aliases checked against the second weapon slot. |
| `banned_weapon.weapon_names` | `["SMG 08/18 Factory", "SMG 08/18 Optical", "SMG 08/18", "SMG08/18"]` | Array of OCR aliases for prohibited SMG 08/18 variants. The displayed category name is fixed in code. |

Each alias must be a string containing a non-whitespace character. Empty arrays
are accepted, but do not reliably disable a restriction: the existing heavy-bomber
confirmation branch has a known issue. Array contents should match the game's
interface language. Configuration does not currently select a server, change
model filenames, add gadget bans, or provide a mode that disables all kicks.

### Maintaining configuration documentation

When a configuration field is added, removed, or changed, update the schema,
typed loader, example configuration, validation tests, and this field reference
in the same change. The test suite checks that every top-level and nested schema field has
a README entry. Keep this README entirely in English.

## Runtime models

Keep these models in `model_weights`:

- `weapon_vehicle_classifier.onnx`
- `player_weapon_ocr.traineddata`

Run the bot from the repository root so its relative paths resolve correctly.
