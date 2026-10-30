use super::*;

fn parse(parameters: &[(&str, Parameter<'_>)]) -> Result<FilesRequest, FilesError> {
    FilesRequest::from_parameters(parameters.iter().copied())
}

#[test]
fn a_name_is_a_relative_path_of_plain_components() {
    for good in [
        "GreetOnKnock.scx",
        "a b.json",
        "x..js",
        ".hidden",
        "a/b",
        "Greet/irp/sources/100/test100.scxml",
    ] {
        assert!(is_file_name(good), "{good}");
    }
    for bad in [
        "", ".", "..", "../x.scx", "a/../b", "a/./b", "a//b", "a/", "a\\b", "/abs", "a\nb", "a\0b",
    ] {
        assert!(!is_file_name(bad), "{bad:?}");
    }
}

#[test]
fn seed_and_name_with_text_describe_a_read_and_a_write() {
    assert_eq!(
        parse(&[("seed", Parameter::Text("a.scx"))]),
        Ok(FilesRequest {
            seed: Some("a.scx".to_owned()),
            write: None
        })
    );
    assert_eq!(
        parse(&[
            ("name", Parameter::Text("a.js")),
            ("text", Parameter::Text("")),
            ("ignored", Parameter::Other),
        ]),
        Ok(FilesRequest {
            seed: None,
            write: Some(("a.js".to_owned(), String::new()))
        })
    );
    assert_eq!(parse(&[]), Ok(FilesRequest::default()));
}

#[test]
fn a_name_that_escapes_the_directory_or_a_lone_name_or_text_is_refused() {
    for name in ["../a.scx", "/behaviors/a.scx", ".."] {
        assert_eq!(
            parse(&[
                ("name", Parameter::Text(name)),
                ("text", Parameter::Text("x"))
            ]),
            Err(FilesError::InvalidName),
            "{name}"
        );
        assert_eq!(
            parse(&[("seed", Parameter::Text(name))]),
            Err(FilesError::InvalidName),
            "{name}"
        );
    }
    assert_eq!(
        parse(&[("seed", Parameter::Other)]),
        Err(FilesError::InvalidName)
    );
    assert_eq!(
        parse(&[("name", Parameter::Text("a.scx"))]),
        Err(FilesError::InvalidText)
    );
    assert_eq!(
        parse(&[("text", Parameter::Text("x"))]),
        Err(FilesError::InvalidText)
    );
    assert_eq!(
        parse(&[
            ("name", Parameter::Text("a.scx")),
            ("text", Parameter::Other)
        ]),
        Err(FilesError::InvalidText)
    );
}

#[test]
fn files_are_in_the_granted_directory() {
    assert_eq!(path("a.scx"), "/behaviors/a.scx");
}

#[test]
fn the_done_data_is_the_examples() {
    assert_eq!(completion(None, Some(2)), r#"{"seed":null,"wrote":2}"#);
    assert_eq!(
        completion(Some("<a b=\"c\">\n\u{1}\\"), None),
        r#"{"seed":"<a b=\"c\">\n\u0001\\","wrote":null}"#
    );
}

#[test]
fn the_package_manifest_binds_the_exact_uri_and_artifacts() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/files.service.json"
    ))
    .expect("the manifest parses");
    manifest.verify(None).expect("the manifest verifies");
    assert_eq!(manifest.service_uri, SERVICE_URI);
    assert_eq!(
        SERVICE_URI,
        format!("https://github.com/stateforward/floppy/services/{SERVICE_NAME}")
    );
    assert_eq!(
        manifest.service_artifact,
        "floppy_service_files.virtualized.wasm"
    );
    assert!(manifest.env_slots.is_empty());
}
