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
            write: None,
            rename: None,
            remove: None,
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
            write: Some(("a.js".to_owned(), String::new())),
            rename: None,
            remove: None,
        })
    );
    assert_eq!(parse(&[]), Ok(FilesRequest::default()));
}

#[test]
fn from_and_to_describe_a_move_and_remove_a_removal() {
    assert_eq!(
        parse(&[
            ("from", Parameter::Text("Greet.draft/Greet.scx")),
            ("to", Parameter::Text("Greet/Greet.scx")),
        ]),
        Ok(FilesRequest {
            seed: None,
            write: None,
            rename: Some((
                "Greet.draft/Greet.scx".to_owned(),
                "Greet/Greet.scx".to_owned()
            )),
            remove: None,
        })
    );
    assert_eq!(
        parse(&[("remove", Parameter::Text("Greet.draft"))]),
        Ok(FilesRequest {
            seed: None,
            write: None,
            rename: None,
            remove: Some("Greet.draft".to_owned()),
        })
    );
    for lone in [
        &[("from", Parameter::Text("a"))][..],
        &[("to", Parameter::Text("a"))][..],
    ] {
        assert_eq!(parse(lone), Err(FilesError::InvalidText), "{lone:?}");
    }
    for name in ["../a.scx", "a\\b"] {
        assert_eq!(
            parse(&[
                ("from", Parameter::Text(name)),
                ("to", Parameter::Text("b"))
            ]),
            Err(FilesError::InvalidName),
            "{name}"
        );
        assert_eq!(
            parse(&[("remove", Parameter::Text(name))]),
            Err(FilesError::InvalidName),
            "{name}"
        );
    }
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

/// A scratch directory standing in for the granted one, its `path()`
/// redirected to it for the duration of the call.
fn granted(test: &str) -> std::path::PathBuf {
    let directory =
        std::env::temp_dir().join(format!("floppy-files-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("the scratch directory is creatable");
    directory
}

fn write_at(directory: &std::path::Path, name: &str, text: &str) {
    let full = directory.join(name);
    std::fs::create_dir_all(full.parent().expect("a parent")).expect("creatable");
    std::fs::write(full, text).expect("writable");
}

#[test]
fn a_move_renames_under_the_directory_and_creates_the_destination() {
    let root = granted("move");
    let _guard = RemoveOnDrop(&root);
    write_at(&root, "Greet.draft/Greet.scx", "<scxml/>");
    rename_under(
        root.to_str().expect("a UTF-8 root"),
        "Greet.draft/Greet.scx",
        "Greet/Greet.scx",
    )
    .expect("the move works");
    assert_eq!(
        std::fs::read_to_string(root.join("Greet/Greet.scx")).expect("readable"),
        "<scxml/>"
    );
    assert!(!root.join("Greet.draft/Greet.scx").exists());
    // A move whose destination names an existing directory is refused, and
    // nothing moves.
    write_at(&root, "Greet.draft/irp/manifest.xml", "<assertions/>");
    let root_name = root.to_str().expect("a UTF-8 root");
    assert_eq!(
        rename_under(root_name, "Greet.draft/irp", "Greet"),
        Err(FilesError::Filesystem)
    );
    assert!(root.join("Greet.draft/irp/manifest.xml").exists());
    // A name that is not a file name is refused, on either side.
    assert_eq!(
        rename_under(root_name, "../outside", "Greet/inside"),
        Err(FilesError::InvalidName)
    );
    assert_eq!(
        rename_under(root_name, "Greet.draft/irp", "/abs"),
        Err(FilesError::InvalidName)
    );
}

#[test]
fn a_remove_takes_a_tree_and_answers_whether_anything_was_there() {
    let root = granted("remove");
    let _guard = RemoveOnDrop(&root);
    let root_name = root.to_str().expect("a UTF-8 root");
    write_at(&root, "Greet/irp/sources/100/test100.scxml", "<scxml/>");
    write_at(&root, "Greet/irp/manifest.xml", "<assertions/>");
    write_at(&root, "Greet/Greet.scx", "<scxml/>");
    std::fs::write(root.join("Greet/link"), "").expect("writable");
    assert_eq!(remove_under(root_name, "Greet"), Ok(true));
    assert!(!root.join("Greet").exists());
    // Nothing there is not a failure.
    assert_eq!(remove_under(root_name, "Greet"), Ok(false));
    assert_eq!(remove_under(root_name, "Greet/Greet.scx"), Ok(false));
    // A name that is not a file name is refused.
    assert_eq!(
        remove_under(root_name, "../outside"),
        Err(FilesError::InvalidName)
    );
}

/// Removes the scratch directory when the test ends, panics or not.
struct RemoveOnDrop<'a>(&'a std::path::Path);

impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0);
    }
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
