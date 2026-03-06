use driver_test_cli::deploy::parse_pnputil_enum_output;

#[test]
fn parse_single_entry_with_date_and_version() {
    let sample = r"Published Name : oem65.inf
Driver Package Provider : NVIDIA
Class : Display adapters
Driver Date and Version : 09/26/2024 31.0.15.4756
Signer Name : Microsoft Windows Hardware Compatibility Publisher";
    let infos = parse_pnputil_enum_output(sample);
    assert_eq!(infos.len(), 1);
    let info = &infos[0];
    assert_eq!(info.published_name.as_deref(), Some("oem65.inf"));
    assert_eq!(info.provider.as_deref(), Some("NVIDIA"));
    assert_eq!(info.class.as_deref(), Some("Display adapters"));
    assert_eq!(info.driver_date.as_deref(), Some("09/26/2024"));
    assert_eq!(info.driver_version.as_deref(), Some("31.0.15.4756"));
}

#[test]
fn parse_multiple_entries() {
    let sample = r"Published Name : oem10.inf
Driver Package Provider : Contoso
Class : System devices
Driver Date and Version : 01/01/2024 2.3.4.5
Signer Name : Contoso Test Signing

Published Name : oem11.inf
Driver Package Provider : Fabrikam
Class : Network adapters
Driver Date and Version : 02/02/2024 9.8.7.6
Signer Name : Microsoft Windows Hardware Compatibility Publisher";
    let infos = parse_pnputil_enum_output(sample);
    assert_eq!(infos.len(), 2);
    assert_eq!(infos[0].published_name.as_deref(), Some("oem10.inf"));
    assert_eq!(infos[1].published_name.as_deref(), Some("oem11.inf"));
}

#[test]
fn parse_handles_missing_fields() {
    let sample = r"Published Name : oem99.inf
Driver Date and Version : 10/10/2024 1.0.0.0";
    let infos = parse_pnputil_enum_output(sample);
    assert_eq!(infos.len(), 1);
    let info = &infos[0];
    assert_eq!(info.provider, None);
    assert_eq!(info.class, None);
    assert_eq!(info.published_name.as_deref(), Some("oem99.inf"));
    assert_eq!(info.driver_version.as_deref(), Some("1.0.0.0"));
}
