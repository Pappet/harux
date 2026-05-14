use harux::hl7::parser::parse_message;
use std::fs;
use std::path::PathBuf;

fn get_fixture_path(sub_dir: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("messages");
    path.push(sub_dir);
    path
}

#[test]
fn test_valid_fixtures() {
    let dir = get_fixture_path("valid");
    let mut count = 0;
    for entry in fs::read_dir(dir).expect("valid messages dir not found") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            count += 1;
            let content = fs::read_to_string(&path).unwrap();
            let msg = parse_message(&content, "127.0.0.1:9999");
            assert!(
                msg.is_ok(),
                "Failed to parse valid fixture {:?}: {:?}",
                path.file_name(),
                msg.err()
            );
            let msg = msg.unwrap();
            assert!(
                msg.parse_error.is_none(),
                "Valid fixture {:?} has parse_error: {:?}",
                path.file_name(),
                msg.parse_error
            );
            // We do not assert that validation_warnings is empty, because real-world valid messages may be missing fields.
        }
    }
    assert!(count > 0, "No valid fixtures found");
}

#[test]
fn test_error_fixtures() {
    let dir = get_fixture_path("errors");
    let mut count = 0;
    for entry in fs::read_dir(dir).expect("error messages dir not found") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            count += 1;
            let content = fs::read_to_string(&path).unwrap();
            let msg = parse_message(&content, "127.0.0.1:9999");
            match msg {
                Ok(m) => {
                    let is_error = m.parse_error.is_some() || !m.validation_warnings.is_empty();
                    assert!(
                        is_error,
                        "Error fixture {:?} parsed cleanly with no warnings or parse errors",
                        path.file_name()
                    );
                }
                Err(_) => {
                    // Failing parse_message is also an acceptable error outcome
                }
            }
        }
    }
    assert!(count > 0, "No error fixtures found");
}

#[test]
fn test_unknown_type_fixtures() {
    let dir = get_fixture_path("unknown_types");
    let mut count = 0;
    for entry in fs::read_dir(dir).expect("unknown_types messages dir not found") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            count += 1;
            let content = fs::read_to_string(&path).unwrap();
            let msg = parse_message(&content, "127.0.0.1:9999");
            match msg {
                Ok(m) => {
                    assert!(
                        m.message_type_description.is_none(),
                        "Unknown type fixture {:?} unexpectedly had a known type",
                        path.file_name()
                    );
                }
                Err(e) => {
                    panic!(
                        "Unknown type fixture {:?} failed to parse completely: {}",
                        path.file_name(),
                        e
                    );
                }
            }
        }
    }
    assert!(count > 0, "No unknown type fixtures found");
}

#[test]
fn test_escape_sequence_pass_through() {
    // Test that escape sequences are preserved literally without being decoded.
    // Note: Full escape decoding (e.g. \E\ -> \) is deferred to a future milestone.
    let raw = "MSH|^~\\&|SENDING|FAC|||2024||ADT^A01|123|P|2.5\rPID|||123||Smith^John\\E\\Doe";
    let msg = parse_message(raw, "127.0.0.1:9999").unwrap();
    let pid = msg.segments.iter().find(|s| s.name == "PID").unwrap();
    let name_field = pid.fields.iter().find(|f| f.index == 5).unwrap();
    assert_eq!(name_field.value, "Smith^John\\E\\Doe");
}

#[test]
fn test_non_default_delimiters() {
    // Using alternative delimiters: MSH*$!#%
    let raw = "MSH*$!#%*SENDING*FAC***2024**ADT$A01*123*P*2.5\rPID***123**Smith$John#E#Doe";
    let msg = parse_message(raw, "127.0.0.1:9999").unwrap();

    assert_eq!(msg.sending_application, "SENDING");
    assert_eq!(msg.message_type, "ADT^A01");
    assert_eq!(msg.message_control_id, "123");

    let pid = msg.segments.iter().find(|s| s.name == "PID").unwrap();
    let name_field = pid.fields.iter().find(|f| f.index == 5).unwrap();
    assert_eq!(name_field.value, "Smith$John#E#Doe");
    // components should be split by $
    assert_eq!(name_field.components[0], "Smith");
    assert_eq!(name_field.components[1], "John#E#Doe");
}

#[test]
fn test_malformed_msh() {
    let raw = "MSH|^~\\&|SENDING";
    let msg = parse_message(raw, "127.0.0.1:9999").unwrap();
    assert!(
        !msg.validation_warnings.is_empty(),
        "Expected validation warnings for malformed MSH"
    );

    let raw2 = "MSH";
    let res2 = parse_message(raw2, "127.0.0.1:9999");
    assert!(res2.is_err(), "Expected parsing error for too short MSH");
}

#[test]
fn test_windows_line_endings() {
    let raw = "MSH|^~\\&|SENDING|FAC|||2024||ADT^A01|123|P|2.5\r\nPID|||123||Smith^John";
    let msg = parse_message(raw, "127.0.0.1:9999").unwrap();
    assert_eq!(msg.segments.len(), 2);
    assert_eq!(msg.segments[1].name, "PID");
}

#[test]
fn test_repeated_pid_3() {
    // Repeated PID-3 using ~.
    // Note: This tests a known limitation. Repetitions are currently not split into
    // separate field objects, so the raw string including the ~ is retained.
    let raw = "MSH|^~\\&|SENDING|FAC|||2024||ADT^A01|123|P|2.5\rPID|||ID1~ID2||Smith^John";
    let msg = parse_message(raw, "127.0.0.1:9999").unwrap();
    assert_eq!(msg.patient_id.unwrap(), "ID1~ID2");
}

#[test]
fn test_trailing_empty_fields() {
    let raw = "MSH|^~\\&|SENDING|FAC|||2024||ADT^A01|123|P|2.5\rPID|||123||Smith^John|||||||||";
    let msg = parse_message(raw, "127.0.0.1:9999").unwrap();
    let pid = msg.segments.iter().find(|s| s.name == "PID").unwrap();
    assert!(pid.fields.len() >= 5);
}

#[test]
fn test_long_message() {
    let mut raw = "MSH|^~\\&|SENDING|FAC|||2024||ADT^A01|123|P|2.5\r".to_string();
    for i in 0..150 {
        raw.push_str(&format!("OBX|{}|NM|TEST||{}||||||F\r", i, i * 2));
    }
    let msg = parse_message(&raw, "127.0.0.1:9999").unwrap();
    assert_eq!(msg.segments.len(), 151); // MSH + 150 OBX
}
