# pnputil Enumeration Parsing Notes

This document describes the parsing strategy implemented in `deploy.rs` for extracting structured driver metadata from `pnputil /enum-drivers` output.

## Goals
- Robust extraction of key fields (Published Name, Provider, Class, Driver Date, Driver Version, Signer Name).
- Support typical Windows 10/11 format lines including combined `Driver Date and Version`.
- Tolerate minor localization changes (e.g., Spanish "nombre publicado") with conservative matching.
- Preserve raw text for diagnostics and future refinement.

## Strategy
1. Split the raw text into logical groups representing individual driver entries. A new group starts when:
   - A line begins with `Published Name` (case-insensitive) OR
   - A blank line is encountered (terminates current group).
2. Within each group, treat any `key : value` line as a candidate. Normalize the key by:
   - Trimming
   - Lowercasing
3. Map normalized keys to struct fields:
   - `published name` -> `published_name`
   - `driver package provider` -> `provider`
   - `class` -> `class`
   - `driver version` -> `driver_version`
   - `driver date` -> `driver_date`
   - `driver date and version` -> split into date + version heuristically
   - `signer name` -> `signer_name`
4. For combined date/version lines, we split whitespace tokens and choose the first token containing `/` as the date and the remainder containing a dot (`.`) as the version (joining remaining tokens to preserve full version build identifiers).
5. Any group with at least one recognized field is retained; unrecognized groups are discarded.

## Selection Heuristic Post-Install
After running `pnputil /add-driver`, we enumerate all drivers and parse them. We attempt to select the **most recent** matching entry whose raw lines contain the original INF file name (case-insensitive). Fallback: attempt to extract `Published Name` directly from the install output if not found in enumeration.

## Known Limitations
- Localization coverage is minimal (only a hint for `nombre publicado`). Additional localized key variants can be added incrementally.
- Driver selection after installation may be ambiguous when multiple entries reference the same INF or when published name differs (e.g., OEM renaming). Future improvement: correlate by timestamp or query specifics using WMI / PnP APIs.
- Date parsing heuristic assumes `/` separated dates; locales using `-` or different ordering may fail to populate `driver_date`.
- Combined date/version lines with additional descriptors (e.g., build metadata words) may result in version including trailing tokens.
- Does not currently parse other useful fields (e.g., `Original Name`, `Provider GUID`). Can be extended easily.

## Future Enhancements
- Add locale key mapping table loaded from a small JSON for broader language support.
- Validate date format and normalize to ISO `YYYY-MM-DD`.
- Incorporate WMI queries (`Win32_PnPSignedDriver`) for authoritative metadata instead of text parsing.
- Provide structured error logging when groups are partially parsed to aid telemetry.
- Distinguish between in-box vs third-party drivers (Signer Name heuristic).

## Testing
Unit tests (`tests/pnputil_parse.rs`) cover:
- Single entry with combined date/version
- Multiple entries separated by blank line
- Entry missing optional fields (provider/class)

Edge cases to add later:
- Localized output sample
- Variant where `Driver Version` appears as a separate line
- Lines with irregular spacing or tabs

## Rationale
Text parsing provides a lightweight, dependency-free first iteration suitable for early integration testing and CI environments. More robust, API-based approaches can replace or augment this parser once the surrounding deployment pipeline stabilizes.

---
Last Updated: 2025-11-13
