//! Declared WASI permissions of a Wasm component package.
//!
//! An extension package or service package manifest may declare the WASI
//! 0.2 interfaces its component imports (`wasi-permissions`). Nothing is
//! ambient: a host links only what a package declares **and** the run grants,
//! and refuses a component that imports a WASI interface its package does
//! not declare. This module is the closed vocabulary both the manifest
//! decoders and the hosts share:
//!
//! | `interface`                    | Fields                     | Grant value at run time            |
//! |--------------------------------|----------------------------|------------------------------------|
//! | `wasi:clocks`                  | none                       | none (the host's wall and monotonic clocks) |
//! | `wasi:filesystem`              | `mount`, `access`          | the host directory mounted at `/<mount>` |
//! | `wasi:random`                  | none                       | none (the host's random sources)   |
//! | `wasi:sockets/ip-name-lookup`  | none                       | none (the host's resolver)         |
//! | `wasi:sockets/tcp`             | none                       | the `IP:PORT` addresses it may connect to |
//!
//! * Preconditions: none; every function accepts untrusted input.
//! * Postconditions: [`WasiPermission::from_json`] accepts exactly one
//!   object of the table above (every other interface, field, or value is
//!   rejected), and [`check_wasi_permissions`] accepts exactly a canonical
//!   list: at most [`MAX_WASI_PERMISSIONS`] entries, strictly ascending by
//!   [`WasiPermission::key`] (so no interface, or filesystem mount, twice).
//! * Invariants: [`WasiPermission::to_canonical_json`] round-trips through
//!   [`WasiPermission::from_json`]; the vocabulary is closed (WASI `cli`,
//!   `http`, and `sockets/udp` are not grantable in this release).
//! * Errors carry no caller-controlled text.

use std::fmt;

/// Maximum entries of one manifest's `wasi-permissions` (a manifest format
/// bound, like its other declared lists).
pub const MAX_WASI_PERMISSIONS: usize = 16;
/// Maximum bytes of one filesystem `mount` name.
pub const MAX_WASI_MOUNT_BYTES: usize = 64;

/// One grantable WASI 0.2 interface (or interface group).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WasiInterface {
    /// `wasi:clocks`: `wall-clock` and `monotonic-clock`.
    Clocks,
    /// `wasi:filesystem`: `types` and `preopens`, one preopened directory.
    Filesystem,
    /// `wasi:random`: `random`, `insecure`, and `insecure-seed`.
    Random,
    /// `wasi:sockets/ip-name-lookup` (with `network` and
    /// `instance-network`).
    SocketsIpNameLookup,
    /// `wasi:sockets/tcp` and `tcp-create-socket` (with `network` and
    /// `instance-network`): outbound connections only.
    SocketsTcp,
}

impl WasiInterface {
    /// Every interface, in canonical order.
    pub const ALL: [Self; 5] = [
        Self::Clocks,
        Self::Filesystem,
        Self::Random,
        Self::SocketsIpNameLookup,
        Self::SocketsTcp,
    ];

    /// The manifest and grant name (`wasi:clocks`, ...).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Clocks => "wasi:clocks",
            Self::Filesystem => "wasi:filesystem",
            Self::Random => "wasi:random",
            Self::SocketsIpNameLookup => "wasi:sockets/ip-name-lookup",
            Self::SocketsTcp => "wasi:sockets/tcp",
        }
    }

    /// The interface named exactly `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|interface| interface.name() == name)
    }
}

impl fmt::Display for WasiInterface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// How a granted directory may be used.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FilesystemAccess {
    /// Read files and list directories only.
    ReadOnly,
    /// Also create, write, rename, and remove.
    ReadWrite,
}

impl FilesystemAccess {
    /// The manifest name (`read-only` or `read-write`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        }
    }
}

/// One declared WASI permission.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WasiPermission {
    /// `wasi:clocks`.
    Clocks,
    /// `wasi:filesystem`: one directory, visible to the component at
    /// `/<mount>`.
    Filesystem {
        /// The mount name ([`is_wasi_mount`]); the run grants the host
        /// directory behind it.
        mount: String,
        /// What the component may do there.
        access: FilesystemAccess,
    },
    /// `wasi:random`.
    Random,
    /// `wasi:sockets/ip-name-lookup`.
    SocketsIpNameLookup,
    /// `wasi:sockets/tcp`: outbound connections to the addresses the run
    /// grants.
    SocketsTcp,
}

/// Why a declared permission or list was rejected. The messages name no
/// caller-controlled value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WasiPermissionError {
    /// An entry is not a JSON object.
    NotAnObject,
    /// An entry's `interface` is missing or not a string.
    MissingInterface,
    /// An entry names an interface outside the closed vocabulary.
    UnknownInterface,
    /// An entry has a field its interface does not take, or lacks one it
    /// requires, or a field is not a string.
    InvalidFields,
    /// A filesystem `mount` is not `[a-z][a-z0-9-]*` within
    /// [`MAX_WASI_MOUNT_BYTES`].
    InvalidMount,
    /// A filesystem `access` is neither `read-only` nor `read-write`.
    InvalidAccess,
    /// The list has more than [`MAX_WASI_PERMISSIONS`] entries.
    TooMany,
    /// The list is not strictly ascending by [`WasiPermission::key`] (an
    /// entry out of canonical order, or one interface or mount twice).
    NotCanonical,
    /// The list is present but empty (an empty list is written by omitting
    /// the field).
    Empty,
}

impl fmt::Display for WasiPermissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotAnObject => "a `wasi-permissions` entry must be a JSON object",
            Self::MissingInterface => {
                "a `wasi-permissions` entry needs a string field `interface`"
            }
            Self::UnknownInterface => {
                "a `wasi-permissions` entry names an unknown interface (the interfaces are wasi:clocks, wasi:filesystem, wasi:random, wasi:sockets/ip-name-lookup, and wasi:sockets/tcp)"
            }
            Self::InvalidFields => {
                "a `wasi-permissions` entry has fields its interface does not take (wasi:filesystem takes exactly `mount` and `access`; the others take none)"
            }
            Self::InvalidMount => {
                "a wasi:filesystem `mount` must be [a-z][a-z0-9-]* of at most 64 bytes"
            }
            Self::InvalidAccess => "a wasi:filesystem `access` must be `read-only` or `read-write`",
            Self::TooMany => "`wasi-permissions` lists more than 16 entries",
            Self::NotCanonical => {
                "`wasi-permissions` must be sorted by interface (then mount) with no entry twice"
            }
            Self::Empty => "`wasi-permissions` must be omitted rather than empty",
        })
    }
}

impl std::error::Error for WasiPermissionError {}

/// Whether `mount` is a filesystem mount name: `[a-z][a-z0-9-]*`, at most
/// [`MAX_WASI_MOUNT_BYTES`] bytes.
#[must_use]
pub fn is_wasi_mount(mount: &str) -> bool {
    mount.len() <= MAX_WASI_MOUNT_BYTES
        && mount
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && mount
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

impl WasiPermission {
    /// The interface it grants.
    #[must_use]
    pub const fn interface(&self) -> WasiInterface {
        match self {
            Self::Clocks => WasiInterface::Clocks,
            Self::Filesystem { .. } => WasiInterface::Filesystem,
            Self::Random => WasiInterface::Random,
            Self::SocketsIpNameLookup => WasiInterface::SocketsIpNameLookup,
            Self::SocketsTcp => WasiInterface::SocketsTcp,
        }
    }

    /// The filesystem mount name, for `wasi:filesystem`.
    #[must_use]
    pub fn mount(&self) -> Option<&str> {
        match self {
            Self::Filesystem { mount, .. } => Some(mount),
            _ => None,
        }
    }

    /// The identity a list is ordered and deduplicated by: the interface,
    /// then the mount.
    #[must_use]
    pub fn key(&self) -> (WasiInterface, Option<&str>) {
        (self.interface(), self.mount())
    }

    /// One `wasi-permissions` entry: `{"interface":"wasi:clocks"}`, or
    /// `{"interface":"wasi:filesystem","mount":M,"access":A}`, with exactly
    /// the fields its interface takes.
    ///
    /// # Errors
    ///
    /// [`WasiPermissionError`] for anything else.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, WasiPermissionError> {
        let serde_json::Value::Object(object) = value else {
            return Err(WasiPermissionError::NotAnObject);
        };
        let Some(serde_json::Value::String(name)) = object.get("interface") else {
            return Err(WasiPermissionError::MissingInterface);
        };
        let interface =
            WasiInterface::from_name(name).ok_or(WasiPermissionError::UnknownInterface)?;
        let text = |field: &str| match object.get(field) {
            Some(serde_json::Value::String(text)) => Ok(text.as_str()),
            _ => Err(WasiPermissionError::InvalidFields),
        };
        let permission = match interface {
            WasiInterface::Clocks => Self::Clocks,
            WasiInterface::Random => Self::Random,
            WasiInterface::SocketsIpNameLookup => Self::SocketsIpNameLookup,
            WasiInterface::SocketsTcp => Self::SocketsTcp,
            WasiInterface::Filesystem => {
                let mount = text("mount")?;
                if !is_wasi_mount(mount) {
                    return Err(WasiPermissionError::InvalidMount);
                }
                let access = match text("access")? {
                    "read-only" => FilesystemAccess::ReadOnly,
                    "read-write" => FilesystemAccess::ReadWrite,
                    _ => return Err(WasiPermissionError::InvalidAccess),
                };
                Self::Filesystem {
                    mount: mount.to_owned(),
                    access,
                }
            }
        };
        let fields = match interface {
            WasiInterface::Filesystem => 3,
            _ => 1,
        };
        if object.len() != fields {
            return Err(WasiPermissionError::InvalidFields);
        }
        Ok(permission)
    }

    /// The entry as canonical compact JSON (`interface` first, then `mount`
    /// and `access`).
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        let text = |value: &str| serde_json::Value::String(value.to_owned()).to_string();
        match self {
            Self::Filesystem { mount, access } => format!(
                "{{\"interface\":{},\"mount\":{},\"access\":{}}}",
                text(self.interface().name()),
                text(mount),
                text(access.name())
            ),
            _ => format!("{{\"interface\":{}}}", text(self.interface().name())),
        }
    }

    /// A one-line description for consent listings (no value is involved:
    /// a declared permission carries none).
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Filesystem { mount, access } => format!(
                "{} (mount `{mount}`, {})",
                self.interface().name(),
                access.name()
            ),
            _ => self.interface().name().to_owned(),
        }
    }
}

impl fmt::Display for WasiPermission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.describe())
    }
}

/// Decodes a manifest's optional `wasi-permissions` field: absent means no
/// permission; present, it must be a non-empty canonical array
/// ([`check_wasi_permissions`]).
///
/// # Errors
///
/// [`WasiPermissionError`] for a non-array, an empty array, an invalid entry,
/// or a non-canonical list.
pub fn wasi_permissions_from_json(
    value: Option<&serde_json::Value>,
) -> Result<Vec<WasiPermission>, WasiPermissionError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let serde_json::Value::Array(items) = value else {
        return Err(WasiPermissionError::NotAnObject);
    };
    if items.is_empty() {
        return Err(WasiPermissionError::Empty);
    }
    if items.len() > MAX_WASI_PERMISSIONS {
        return Err(WasiPermissionError::TooMany);
    }
    let permissions = items
        .iter()
        .map(WasiPermission::from_json)
        .collect::<Result<Vec<_>, _>>()?;
    check_wasi_permissions(&permissions)?;
    Ok(permissions)
}

/// The canonical `,"wasi-permissions":[...]` manifest suffix, or nothing for
/// an empty list (the field is omitted).
#[must_use]
pub fn wasi_permissions_canonical_field(permissions: &[WasiPermission]) -> String {
    if permissions.is_empty() {
        return String::new();
    }
    let entries: Vec<String> = permissions
        .iter()
        .map(WasiPermission::to_canonical_json)
        .collect();
    format!(",\"wasi-permissions\":[{}]", entries.join(","))
}

/// Checks a declared list: at most [`MAX_WASI_PERMISSIONS`] valid entries,
/// strictly ascending by [`WasiPermission::key`].
///
/// # Errors
///
/// [`WasiPermissionError::TooMany`], [`WasiPermissionError::InvalidMount`],
/// or [`WasiPermissionError::NotCanonical`].
pub fn check_wasi_permissions(permissions: &[WasiPermission]) -> Result<(), WasiPermissionError> {
    if permissions.len() > MAX_WASI_PERMISSIONS {
        return Err(WasiPermissionError::TooMany);
    }
    if permissions
        .iter()
        .filter_map(WasiPermission::mount)
        .any(|mount| !is_wasi_mount(mount))
    {
        return Err(WasiPermissionError::InvalidMount);
    }
    if permissions
        .windows(2)
        .any(|pair| matches!(pair, [left, right] if left.key() >= right.key()))
    {
        return Err(WasiPermissionError::NotCanonical);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Vec<WasiPermission>, WasiPermissionError> {
        let value: serde_json::Value = serde_json::from_str(text).expect("test JSON parses");
        wasi_permissions_from_json(Some(&value))
    }

    #[test]
    fn every_interface_round_trips_canonically() {
        let list = parse(
            r#"[{"interface":"wasi:clocks"},{"interface":"wasi:filesystem","mount":"a","access":"read-only"},{"interface":"wasi:filesystem","mount":"b","access":"read-write"},{"interface":"wasi:random"},{"interface":"wasi:sockets/ip-name-lookup"},{"interface":"wasi:sockets/tcp"}]"#,
        )
        .expect("the full vocabulary parses");
        assert_eq!(list.len(), 6);
        let field = wasi_permissions_canonical_field(&list);
        let reparsed = parse(field.trim_start_matches(",\"wasi-permissions\":"))
            .expect("the canonical field reparses");
        assert_eq!(reparsed, list);
        assert_eq!(list[1].describe(), "wasi:filesystem (mount `a`, read-only)");
    }

    #[test]
    fn absent_means_none_and_empty_is_rejected() {
        assert_eq!(wasi_permissions_from_json(None), Ok(Vec::new()));
        assert_eq!(parse("[]"), Err(WasiPermissionError::Empty));
        assert_eq!(wasi_permissions_canonical_field(&[]), "");
    }

    #[test]
    fn unknown_interfaces_fields_and_values_fail_closed() {
        for (text, error) in [
            (
                r#"[{"interface":"wasi:http/outgoing-handler"}]"#,
                WasiPermissionError::UnknownInterface,
            ),
            (
                r#"[{"interface":"wasi:cli/environment"}]"#,
                WasiPermissionError::UnknownInterface,
            ),
            (
                r#"[{"interface":"WASI:CLOCKS"}]"#,
                WasiPermissionError::UnknownInterface,
            ),
            (r#"["wasi:clocks"]"#, WasiPermissionError::NotAnObject),
            (
                r#"{"interface":"wasi:clocks"}"#,
                WasiPermissionError::NotAnObject,
            ),
            (r"[{}]", WasiPermissionError::MissingInterface),
            (
                r#"[{"interface":"wasi:clocks","mount":"a"}]"#,
                WasiPermissionError::InvalidFields,
            ),
            (
                r#"[{"interface":"wasi:filesystem","mount":"a"}]"#,
                WasiPermissionError::InvalidFields,
            ),
            (
                r#"[{"interface":"wasi:filesystem","mount":"a","access":"read-write","x":1}]"#,
                WasiPermissionError::InvalidFields,
            ),
            (
                r#"[{"interface":"wasi:filesystem","mount":"A","access":"read-write"}]"#,
                WasiPermissionError::InvalidMount,
            ),
            (
                r#"[{"interface":"wasi:filesystem","mount":"a","access":"write"}]"#,
                WasiPermissionError::InvalidAccess,
            ),
            (
                r#"[{"interface":"wasi:random"},{"interface":"wasi:clocks"}]"#,
                WasiPermissionError::NotCanonical,
            ),
            (
                r#"[{"interface":"wasi:clocks"},{"interface":"wasi:clocks"}]"#,
                WasiPermissionError::NotCanonical,
            ),
            (
                r#"[{"interface":"wasi:filesystem","mount":"a","access":"read-only"},{"interface":"wasi:filesystem","mount":"a","access":"read-write"}]"#,
                WasiPermissionError::NotCanonical,
            ),
        ] {
            assert_eq!(parse(text), Err(error), "{text}");
        }
    }

    #[test]
    fn the_list_is_bounded() {
        let entries: Vec<String> = (0..=MAX_WASI_PERMISSIONS)
            .map(|index| {
                format!(
                    r#"{{"interface":"wasi:filesystem","mount":"m{index:02}","access":"read-only"}}"#
                )
            })
            .collect();
        assert_eq!(
            parse(&format!("[{}]", entries.join(","))),
            Err(WasiPermissionError::TooMany)
        );
        assert!(parse(&format!("[{}]", entries[..MAX_WASI_PERMISSIONS].join(","))).is_ok());
    }

    #[test]
    fn mount_names_are_bounded_lowercase_identifiers() {
        assert!(is_wasi_mount("workspace"));
        assert!(is_wasi_mount("a-1"));
        assert!(!is_wasi_mount(""));
        assert!(!is_wasi_mount("1a"));
        assert!(!is_wasi_mount("a/b"));
        assert!(!is_wasi_mount(&"a".repeat(MAX_WASI_MOUNT_BYTES + 1)));
        assert!(is_wasi_mount(&"a".repeat(MAX_WASI_MOUNT_BYTES)));
    }
}
