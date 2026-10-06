# Configuration maintenance

- Keep README.md entirely in English.
- Document the selected JSON configuration file (`--config`, default `config.json`
  in the current working directory) as the sole source of application settings.
- When adding, removing, or changing a configuration field, update config.schema.json,
  the typed configuration loader, config.example.json, the README field reference,
  and the relevant validation tests in the same change.
- Optional configuration fields must work when absent and be schema-validated when present.
- Validate the entire configuration before network requests or monitoring start.
- Only sid, remid, and bf1_path are mandatory startup fields. Apply optional
  defaults from the schema. Missing recognition regions/colours must be collected
  and saved before any monitoring workers or kick processing start.
- Missing, null, or empty webhook settings disable their requests entirely.
- Never commit real config.json credentials or include their values in diagnostics.
- Route every fatal error, including authentication/runtime failures and Rust panics,
  through error reporting that waits for Enter before exiting. Closed standard input
  may exit immediately. Never dump credentials or HTTP headers in panic diagnostics.
