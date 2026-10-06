//! Exact-namespace schema contracts and deterministic local resolvers.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use scxml_syntax::Document;
use scxml_syntax::telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use sha2::{Digest, Sha256};

use crate::request::{MAX_COMPONENT_METADATA_BYTES, MAX_COMPONENT_SCHEMA_BYTES};
use crate::{Diagnostic, Report};

/// Exact WIT world supported for extension namespace extension components.
pub const SUPPORTED_NAMESPACE_WIT_WORLD: &str = "stateforward:scxml/executable-content-extension";
/// Exact WIT package version supported for extension components
/// components.
pub const SUPPORTED_NAMESPACE_WIT_VERSION: &str = "0.2.0";
/// Maximum accepted UTF-8 bytes in one extension package manifest field.
pub const MAX_EXTENSION_PACKAGE_FIELD_BYTES: usize = MAX_COMPONENT_METADATA_BYTES;
/// Maximum entries of a manifest's `env-slots` and of its `http-origins`.
pub const MAX_EXTENSION_PACKAGE_LIST_ITEMS: usize = 32;
/// Maximum bytes of one `env-slots` entry.
pub const MAX_ENV_SLOT_BYTES: usize = 64;
/// Maximum bytes of one `http-origins` entry.
pub const MAX_HTTP_ORIGIN_BYTES: usize = 2_048;
/// The only extension package manifest format (its `format` field) this
/// release reads.
pub const EXTENSION_PACKAGE_FORMAT: &str = "stateforward.scxml.extension-package/v2";
/// Manifest fields in their canonical order (`format` first).
const EXTENSION_PACKAGE_FIELDS: [&str; 10] = [
    "format",
    "namespace-uri",
    "package-version",
    "extension-artifact",
    "schema-artifact",
    "schema-sha256",
    "wit-world",
    "wit-version",
    "http-origins",
    "env-slots",
];

/// Whether `slot` is an `env-slots` entry: `[a-z][a-z0-9-]*`, at most
/// [`MAX_ENV_SLOT_BYTES`] bytes, never ending in `expr` (the suffix of the
/// expression form, `env:<slot>expr`).
#[must_use]
pub fn is_env_slot(slot: &str) -> bool {
    slot.len() <= MAX_ENV_SLOT_BYTES
        && !slot.ends_with("expr")
        && slot
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && slot
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Whether `origin` is an `http-origins` entry as this crate can judge it
/// without a URL parser: `http://` or `https://` and a non-empty rest of at
/// most [`MAX_HTTP_ORIGIN_BYTES`] bytes with no white space, control
/// character, userinfo (`@`), query (`?`), fragment (`#`), or backslash.
/// Hosts canonicalize it with the WHATWG URL parser before use and refuse
/// what that rejects.
#[must_use]
pub fn is_http_origin(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"));
    origin.len() <= MAX_HTTP_ORIGIN_BYTES
        && rest.is_some_and(|rest| {
            !rest.is_empty()
                && !rest.starts_with('/')
                && rest.chars().all(|character| {
                    !character.is_whitespace()
                        && !character.is_control()
                        && !matches!(character, '@' | '?' | '#' | '\\')
                })
        })
}

/// Policy when a used namespace has no registered schema.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MissingSchemaPolicy {
    /// Emit an error for each missing namespace.
    #[default]
    Strict,
    /// Skip missing namespaces while validating every resolved schema.
    Lax,
}

/// Declarative package binding for one extension namespace.
///
/// Validators and compilers consume this metadata without instantiating the
/// extension component. Artifact paths are package-relative identifiers; schema
/// integrity is checked against lowercase SHA-256 hex.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionPackageManifest {
    /// Exact namespace URI used for schema and extension component lookup.
    pub namespace_uri: String,
    /// Package version selected by the composition root.
    pub package_version: String,
    /// Extension component artifact identifier.
    pub extension_artifact: String,
    /// Schema artifact identifier.
    pub schema_artifact: String,
    /// Lowercase SHA-256 digest of the schema bytes.
    pub schema_sha256: String,
    /// WIT world implemented by the extension component.
    pub wit_world: String,
    /// WIT package/world version.
    pub wit_version: String,
    /// The `env` slots its elements accept (`env:<slot>` attributes): at most
    /// [`MAX_EXTENSION_PACKAGE_LIST_ITEMS`] distinct [`is_env_slot`] entries.
    pub env_slots: Vec<String>,
    /// The origins or URL prefixes its extension component's requests may
    /// reach: at most [`MAX_EXTENSION_PACKAGE_LIST_ITEMS`] distinct
    /// [`is_http_origin`] entries. Binding the package consents to them.
    pub http_origins: Vec<String>,
}

impl ExtensionPackageManifest {
    /// Parses one [`EXTENSION_PACKAGE_FORMAT`] manifest: a JSON object with
    /// exactly the manifest fields, every value a string except
    /// `http-origins` and `env-slots` (arrays of at most
    /// [`MAX_EXTENSION_PACKAGE_LIST_ITEMS`] strings), and `format` equal to
    /// [`EXTENSION_PACKAGE_FORMAT`]. The entries themselves are judged by
    /// [`Self::verify_schema`]. Deterministic and side-effect-free; records
    /// no telemetry.
    ///
    /// # Errors
    ///
    /// A message naming the offending field, never its value.
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| "it is not JSON".to_owned())?;
        let serde_json::Value::Object(object) = value else {
            return Err("it is not a JSON object".into());
        };
        if let Some(unknown) = object
            .keys()
            .find(|key| !EXTENSION_PACKAGE_FIELDS.contains(&key.as_str()))
        {
            return Err(format!(
                "unknown field `{}` (the fields are {})",
                unknown.escape_debug(),
                EXTENSION_PACKAGE_FIELDS.join(", ")
            ));
        }
        let field = |name: &str| -> Result<String, String> {
            match object.get(name) {
                Some(serde_json::Value::String(value)) => Ok(value.clone()),
                Some(_) => Err(format!("field `{name}` must be a string")),
                None => Err(format!("missing field `{name}`")),
            }
        };
        let list = |name: &str| -> Result<Vec<String>, String> {
            match object.get(name) {
                Some(serde_json::Value::Array(items))
                    if items.len() <= MAX_EXTENSION_PACKAGE_LIST_ITEMS =>
                {
                    items
                        .iter()
                        .map(|item| match item {
                            serde_json::Value::String(text) => Ok(text.clone()),
                            _ => Err(format!("field `{name}` must be an array of strings")),
                        })
                        .collect()
                }
                Some(serde_json::Value::Array(_)) => Err(format!(
                    "field `{name}` lists more than {MAX_EXTENSION_PACKAGE_LIST_ITEMS} entries"
                )),
                Some(_) => Err(format!("field `{name}` must be an array of strings")),
                None => Err(format!("missing field `{name}`")),
            }
        };
        if field("format")? != EXTENSION_PACKAGE_FORMAT {
            return Err(format!(
                "field `format` must be `{EXTENSION_PACKAGE_FORMAT}`"
            ));
        }
        Ok(Self {
            namespace_uri: field("namespace-uri")?,
            package_version: field("package-version")?,
            extension_artifact: field("extension-artifact")?,
            schema_artifact: field("schema-artifact")?,
            schema_sha256: field("schema-sha256")?,
            wit_world: field("wit-world")?,
            wit_version: field("wit-version")?,
            env_slots: list("env-slots")?,
            http_origins: list("http-origins")?,
        })
    }

    /// Verifies bounded identity, supported WIT compatibility, and schema integrity.
    ///
    /// This check is deterministic and side-effect-free, and records no
    /// telemetry. It does not read or instantiate `extension_artifact`.
    ///
    /// # Errors
    ///
    /// Returns a stable message when a required field is empty or oversized,
    /// the WIT identity is unsupported, or schema identity/integrity does not
    /// match the supplied schema bytes. Errors never include caller-controlled
    /// field values.
    pub fn verify_schema(&self, schema: &SchemaSource) -> Result<(), String> {
        for (name, value) in [
            ("namespace-uri", self.namespace_uri.as_str()),
            ("package-version", self.package_version.as_str()),
            ("extension-artifact", self.extension_artifact.as_str()),
            ("schema-artifact", self.schema_artifact.as_str()),
            ("schema-sha256", self.schema_sha256.as_str()),
            ("wit-world", self.wit_world.as_str()),
            ("wit-version", self.wit_version.as_str()),
        ] {
            if value.is_empty() {
                return Err(format!(
                    "extension package field `{name}` must not be empty"
                ));
            }
            if value.len() > MAX_EXTENSION_PACKAGE_FIELD_BYTES {
                return Err(format!(
                    "extension package field `{name}` exceeds {MAX_EXTENSION_PACKAGE_FIELD_BYTES} bytes"
                ));
            }
        }
        for (name, items, valid) in [
            (
                "env-slots",
                &self.env_slots,
                is_env_slot as fn(&str) -> bool,
            ),
            ("http-origins", &self.http_origins, is_http_origin),
        ] {
            if items.len() > MAX_EXTENSION_PACKAGE_LIST_ITEMS {
                return Err(format!(
                    "extension package field `{name}` lists more than {MAX_EXTENSION_PACKAGE_LIST_ITEMS} entries"
                ));
            }
            for (index, item) in items.iter().enumerate() {
                if !valid(item) {
                    return Err(format!(
                        "extension package field `{name}` has an invalid entry"
                    ));
                }
                if items[..index].contains(item) {
                    return Err(format!(
                        "extension package field `{name}` lists an entry twice"
                    ));
                }
            }
        }
        if self.wit_world != SUPPORTED_NAMESPACE_WIT_WORLD {
            return Err(format!(
                "extension package WIT world must equal `{SUPPORTED_NAMESPACE_WIT_WORLD}`"
            ));
        }
        if self.wit_version != SUPPORTED_NAMESPACE_WIT_VERSION {
            return Err(format!(
                "extension package WIT version must equal `{SUPPORTED_NAMESPACE_WIT_VERSION}`"
            ));
        }
        if schema.system_id.is_empty() {
            return Err("schema system-id must not be empty".to_owned());
        }
        if schema.system_id.len() > MAX_COMPONENT_METADATA_BYTES {
            return Err(format!(
                "schema system-id exceeds {MAX_COMPONENT_METADATA_BYTES} bytes"
            ));
        }
        if schema.bytes.len() > MAX_COMPONENT_SCHEMA_BYTES {
            return Err(format!(
                "schema bytes exceed {MAX_COMPONENT_SCHEMA_BYTES} bytes"
            ));
        }
        if schema.system_id != self.schema_artifact {
            return Err("schema artifact mismatch for extension package".to_owned());
        }
        let actual = hex::encode(Sha256::digest(&schema.bytes));
        if actual != self.schema_sha256 {
            return Err("schema digest mismatch for extension package".to_owned());
        }
        Ok(())
    }
}

/// Resolved XSD source independent of the backend parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaSource {
    /// Stable local identifier used as the XSD base URI.
    pub system_id: String,
    /// Raw XSD bytes.
    pub bytes: Vec<u8>,
}

impl SchemaSource {
    /// Creates a schema source.
    #[must_use]
    pub fn new(system_id: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            system_id: system_id.into(),
            bytes: bytes.into(),
        }
    }
}

/// Schema resolution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaResolveError {
    /// No mapping exists for the exact namespace URI.
    NotFound,
    /// A registered local resource could not be loaded.
    Load(String),
}

impl fmt::Display for SchemaResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("schema is not registered"),
            Self::Load(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SchemaResolveError {}

/// Resolves one XSD by exact namespace URI.
///
/// Implementations are expected to be deterministic. The validator never
/// performs ambient network access; remote loaders require a separate explicit
/// capability not present in this contract.
pub trait SchemaResolver {
    /// Resolves the exact namespace URI.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaResolveError::NotFound`] or a deterministic local load
    /// failure.
    fn resolve(&self, namespace_uri: &str) -> Result<SchemaSource, SchemaResolveError>;

    /// Resolves every schema document that together governs `namespace_uri`.
    ///
    /// A namespace may be defined by a multi-document schema set (for example
    /// the official W3C SCXML schema driver and its included modules). The
    /// default returns the single [`Self::resolve`] result.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaResolveError::NotFound`] or a deterministic local load
    /// failure.
    fn resolve_documents(
        &self,
        namespace_uri: &str,
    ) -> Result<Vec<SchemaSource>, SchemaResolveError> {
        self.resolve(namespace_uri).map(|source| vec![source])
    }
}

/// In-memory resolver suitable for embedded schemas and tests.
#[derive(Clone, Debug, Default)]
pub struct MemorySchemaResolver {
    schemas: BTreeMap<String, SchemaSource>,
}

impl MemorySchemaResolver {
    /// Creates an empty resolver.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            schemas: BTreeMap::new(),
        }
    }

    /// Registers or replaces one exact namespace mapping.
    pub fn insert(&mut self, namespace_uri: impl Into<String>, source: SchemaSource) {
        self.schemas.insert(namespace_uri.into(), source);
    }
}

impl SchemaResolver for MemorySchemaResolver {
    fn resolve(&self, namespace_uri: &str) -> Result<SchemaSource, SchemaResolveError> {
        self.schemas
            .get(namespace_uri)
            .cloned()
            .ok_or(SchemaResolveError::NotFound)
    }
}

/// Filesystem mapping resolver. Paths must be registered explicitly.
///
/// Resolution accepts only regular, non-symbolic-link files with bounded path
/// text and bounded declared and observed byte lengths. It performs no network
/// access and does not interpret schema bytes or instantiate the extension component.
#[derive(Clone, Debug, Default)]
pub struct FileSchemaResolver {
    schemas: BTreeMap<String, PathBuf>,
}

impl FileSchemaResolver {
    /// Creates an empty resolver.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            schemas: BTreeMap::new(),
        }
    }

    /// Registers an exact namespace URI to a local path.
    pub fn insert(&mut self, namespace_uri: impl Into<String>, path: impl Into<PathBuf>) {
        self.schemas.insert(namespace_uri.into(), path.into());
    }

    /// Returns the registered path for inspection.
    #[must_use]
    pub fn path(&self, namespace_uri: &str) -> Option<&Path> {
        self.schemas.get(namespace_uri).map(PathBuf::as_path)
    }
}

impl SchemaResolver for FileSchemaResolver {
    fn resolve(&self, namespace_uri: &str) -> Result<SchemaSource, SchemaResolveError> {
        self.resolve_with_before_open_hook(namespace_uri, || {})
    }
}

impl FileSchemaResolver {
    /// Opens the registered schema path without following symbolic links.
    ///
    /// Unix opens with `O_NOFOLLOW`; Windows opens with `FILE_FLAG_OPEN_REPARSE_POINT`;
    /// other platforms fail closed because they cannot guarantee no-follow opens.
    /// The caller maps failures to the exact symlink-vs-unopenable diagnostic.
    fn open_schema_file(path: &Path) -> Result<std::fs::File, std::io::Error> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "platform cannot open schemas without following symbolic links",
            ))
        }
    }

    fn resolve_with_before_open_hook(
        &self,
        namespace_uri: &str,
        before_open: impl FnOnce(),
    ) -> Result<SchemaSource, SchemaResolveError> {
        // An unregistered namespace is a map miss, not I/O: no span opens.
        let path = self
            .schemas
            .get(namespace_uri)
            .ok_or(SchemaResolveError::NotFound)?;
        let span = observe(Module::Validator, Operation::ValidatorSchemaFileRead);
        let result = (|| {
            let system_id = path.to_string_lossy();
            if system_id.len() > MAX_COMPONENT_METADATA_BYTES {
                return Err(SchemaResolveError::Load(format!(
                    "registered schema system-id exceeds {MAX_COMPONENT_METADATA_BYTES} bytes"
                )));
            }
            let path_metadata = std::fs::symlink_metadata(path).map_err(|_| {
                SchemaResolveError::Load("registered schema metadata is unavailable".to_owned())
            })?;
            if path_metadata.file_type().is_symlink() {
                return Err(SchemaResolveError::Load(
                    "registered schema path must not be a symbolic link".to_owned(),
                ));
            }
            if !path_metadata.is_file() {
                return Err(SchemaResolveError::Load(
                    "registered schema path must identify a regular file".to_owned(),
                ));
            }
            let maximum_schema_bytes = u64::try_from(MAX_COMPONENT_SCHEMA_BYTES).map_err(|_| {
                SchemaResolveError::Load("schema byte limit is not representable".to_owned())
            })?;
            if path_metadata.len() > maximum_schema_bytes {
                return Err(SchemaResolveError::Load(format!(
                    "registered schema exceeds {MAX_COMPONENT_SCHEMA_BYTES} bytes"
                )));
            }
            before_open();
            let file = Self::open_schema_file(path).map_err(|_| {
                if std::fs::symlink_metadata(path)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    SchemaResolveError::Load(
                        "registered schema path must not be a symbolic link".to_owned(),
                    )
                } else {
                    SchemaResolveError::Load("registered schema could not be opened".to_owned())
                }
            })?;
            let file_metadata = file.metadata().map_err(|_| {
                SchemaResolveError::Load("opened schema metadata is unavailable".to_owned())
            })?;
            if file_metadata.file_type().is_symlink() {
                return Err(SchemaResolveError::Load(
                    "opened schema must not be a symbolic link".to_owned(),
                ));
            }
            if !file_metadata.is_file() {
                return Err(SchemaResolveError::Load(
                    "opened schema must remain a regular file".to_owned(),
                ));
            }
            if file_metadata.len() > maximum_schema_bytes {
                return Err(SchemaResolveError::Load(format!(
                    "opened schema exceeds {MAX_COMPONENT_SCHEMA_BYTES} bytes"
                )));
            }
            let read_limit = maximum_schema_bytes.checked_add(1).ok_or_else(|| {
                SchemaResolveError::Load("schema read limit is not representable".to_owned())
            })?;
            let initial_capacity = usize::try_from(file_metadata.len())
                .unwrap_or(MAX_COMPONENT_SCHEMA_BYTES)
                .min(MAX_COMPONENT_SCHEMA_BYTES);
            let bytes = read_bounded_schema(file, read_limit, initial_capacity)?;
            Ok(SchemaSource::new(system_id.into_owned(), bytes))
        })();
        if result.is_err() {
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::InputRead);
        } else {
            span.event(SignalEvent::ExternalReadCompleted);
        }
        result
    }
}

fn read_bounded_schema(
    reader: impl Read,
    read_limit: u64,
    initial_capacity: usize,
) -> Result<Vec<u8>, SchemaResolveError> {
    let mut bytes = Vec::with_capacity(initial_capacity.min(MAX_COMPONENT_SCHEMA_BYTES));
    reader
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|_| SchemaResolveError::Load("registered schema read failed".to_owned()))?;
    if bytes.len() > MAX_COMPONENT_SCHEMA_BYTES {
        return Err(SchemaResolveError::Load(format!(
            "observed schema bytes exceed {MAX_COMPONENT_SCHEMA_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

/// Resolver decorator caching both successes and failures by exact URI.
///
/// # Mutation and sharing
///
/// The cache is per instance: it lives exactly as long as this value, starts
/// empty, is never shared with another instance, and is never evicted.
/// [`SchemaResolver::resolve`] mutates it through `&self` (a `RefCell`),
/// inserting one entry, success or failure, for each distinct namespace URI
/// the first time that URI is resolved; later calls for the same URI return
/// the cached result without calling the inner resolver.
/// [`SchemaResolver::resolve_documents`] is delegated uncached. The cache
/// borrow is released before the inner resolver runs, so an inner resolver
/// that resolves through this same instance does not panic. Because of the
/// `RefCell`, the type is `!Sync` (and `Send` exactly when `R` is): share one
/// instance within a single thread, move it between threads, or give each
/// thread its own instance and therefore its own cache.
///
/// ```compile_fail
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<scxml_validator_core::CachingSchemaResolver<()>>();
/// ```
#[derive(Debug)]
pub struct CachingSchemaResolver<R> {
    inner: R,
    cache: RefCell<BTreeMap<String, Result<SchemaSource, SchemaResolveError>>>,
}

impl<R> CachingSchemaResolver<R> {
    /// Wraps a resolver with a deterministic cache owned by this instance.
    #[must_use]
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            cache: RefCell::new(BTreeMap::new()),
        }
    }

    /// Returns the number of exact namespace entries cached.
    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.cache.borrow().len()
    }
}

impl<R: SchemaResolver> SchemaResolver for CachingSchemaResolver<R> {
    fn resolve_documents(
        &self,
        namespace_uri: &str,
    ) -> Result<Vec<SchemaSource>, SchemaResolveError> {
        // Multi-document sets are delegated uncached so a decorated resolver
        // (for example a bundled-schema fallback) keeps its set semantics.

        self.inner.resolve_documents(namespace_uri)
    }

    fn resolve(&self, namespace_uri: &str) -> Result<SchemaSource, SchemaResolveError> {
        if let Some(cached) = self.cache.borrow().get(namespace_uri) {
            return cached.clone();
        }
        let resolved = self.inner.resolve(namespace_uri);
        self.cache
            .borrow_mut()
            .insert(namespace_uri.to_owned(), resolved.clone());
        resolved
    }
}

/// Backend-independent schema validation interface.
pub trait SchemaValidator {
    /// Validates a parsed document against the resolved schema set.
    ///
    /// # Errors
    ///
    /// Backend setup and schema compilation failures are returned separately;
    /// instance validation failures belong in the returned [`Report`].
    fn validate(
        &self,
        document: &Document,
        schemas: &[SchemaSource],
    ) -> Result<Report, SchemaBackendError>;
}

/// XSD backend setup or compilation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaBackendError {
    /// Stable backend failure code.
    pub code: String,
    /// Safe human-readable message.
    pub message: String,
    /// Instance-specific actionable help; when absent, the explain table's
    /// generic help for [`Self::code`] applies.
    pub help: Option<String>,
}

impl SchemaBackendError {
    /// Creates a backend failure.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            help: None,
        }
    }

    /// Attaches instance-specific actionable help.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Converts this setup failure into a source diagnostic.
    #[must_use]
    pub fn into_diagnostic(self, document: &Document) -> Diagnostic {
        let diagnostic = Diagnostic::error(self.code, self.message, document.root.start_tag_span);
        match self.help {
            Some(help) => diagnostic.with_help(help),
            None => diagnostic,
        }
    }
}

impl fmt::Display for SchemaBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for SchemaBackendError {}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs::{self, File};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use scxml_syntax::telemetry::{Recorder, install_recorder};

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "scxml-validator-core-schema-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("isolated schema test directory must be created");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct CountingResolver {
        calls: Rc<Cell<usize>>,
    }

    impl SchemaResolver for CountingResolver {
        fn resolve(&self, namespace_uri: &str) -> Result<SchemaSource, SchemaResolveError> {
            self.calls.set(self.calls.get() + 1);
            Ok(SchemaSource::new(namespace_uri, b"schema".to_vec()))
        }
    }

    #[test]
    fn cache_is_exact_and_deterministic() {
        let calls = Rc::new(Cell::new(0));
        let resolver = CachingSchemaResolver::new(CountingResolver {
            calls: Rc::clone(&calls),
        });
        assert!(resolver.resolve("urn:a").is_ok());
        assert!(resolver.resolve("urn:a").is_ok());
        assert!(resolver.resolve("urn:a/").is_ok());
        assert_eq!(calls.get(), 2);
        assert_eq!(resolver.cache_len(), 2);
    }

    #[test]
    fn file_resolver_rejects_sparse_oversize_before_reading() {
        let directory = TestDirectory::new();
        let path = directory.join("oversize.xsd");
        let file = File::create(&path).expect("sparse schema fixture must be created");
        file.set_len(
            u64::try_from(MAX_COMPONENT_SCHEMA_BYTES).expect("schema byte limit must fit in u64")
                + 1,
        )
        .expect("sparse schema fixture length must be set");
        let mut resolver = FileSchemaResolver::new();
        resolver.insert("urn:oversize", path);

        assert_eq!(
            resolver.resolve("urn:oversize"),
            Err(SchemaResolveError::Load(format!(
                "registered schema exceeds {MAX_COMPONENT_SCHEMA_BYTES} bytes"
            )))
        );
    }

    #[test]
    fn bounded_schema_read_observes_limit_plus_one() {
        let limit = u64::try_from(MAX_COMPONENT_SCHEMA_BYTES)
            .expect("schema byte limit must fit in u64")
            + 1;
        assert_eq!(
            read_bounded_schema(std::io::repeat(0), limit, MAX_COMPONENT_SCHEMA_BYTES),
            Err(SchemaResolveError::Load(format!(
                "observed schema bytes exceed {MAX_COMPONENT_SCHEMA_BYTES} bytes"
            )))
        );
    }

    #[test]
    fn file_resolver_rejects_nonregular_path() {
        let directory = TestDirectory::new();
        let mut resolver = FileSchemaResolver::new();
        resolver.insert("urn:directory", &directory.0);

        assert_eq!(
            resolver.resolve("urn:directory"),
            Err(SchemaResolveError::Load(
                "registered schema path must identify a regular file".to_owned()
            ))
        );
    }

    #[test]
    fn file_resolver_rejects_oversized_system_id_before_metadata_access() {
        let oversized = "x".repeat(MAX_COMPONENT_METADATA_BYTES + 1);
        let mut resolver = FileSchemaResolver::new();
        resolver.insert("urn:oversized-system-id", oversized);

        assert_eq!(
            resolver.resolve("urn:oversized-system-id"),
            Err(SchemaResolveError::Load(format!(
                "registered schema system-id exceeds {MAX_COMPONENT_METADATA_BYTES} bytes"
            )))
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_resolver_rejects_symbolic_link() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new();
        let target = directory.join("target.xsd");
        fs::write(&target, b"schema").expect("schema target fixture must be written");
        let link = directory.join("link.xsd");
        symlink(&target, &link).expect("schema symlink fixture must be created");
        let mut resolver = FileSchemaResolver::new();
        resolver.insert("urn:symlink", link);

        assert_eq!(
            resolver.resolve("urn:symlink"),
            Err(SchemaResolveError::Load(
                "registered schema path must not be a symbolic link".to_owned()
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_resolver_rejects_path_replaced_with_symlink_before_open() {
        use scxml_syntax::telemetry::{
            ErrorCategory, Module, Operation, Recorder, SignalEvent, install_recorder,
        };
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new();
        let registered = directory.join("registered.xsd");
        let replacement = directory.join("replacement.xsd");
        fs::write(&registered, b"registered").expect("registered schema fixture must be written");
        fs::write(&replacement, b"replacement")
            .expect("replacement schema fixture must be written");
        let mut resolver = FileSchemaResolver::new();
        resolver.insert("urn:replacement-race", &registered);
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());

        let result = resolver.resolve_with_before_open_hook("urn:replacement-race", || {
            fs::remove_file(&registered).expect("registered schema fixture must be removed");
            symlink(&replacement, &registered)
                .expect("replacement symlink fixture must be created");
        });

        assert_eq!(
            result,
            Err(SchemaResolveError::Load(
                "registered schema path must not be a symbolic link".to_owned()
            ))
        );
        assert!(recorder.observations().iter().any(|observation| {
            observation.module == Module::Validator
                && observation.operation == Operation::ValidatorSchemaFileRead
                && observation.error == Some(ErrorCategory::InputRead)
                && observation.events.contains(&SignalEvent::InputRejected)
        }));
    }

    #[test]
    fn package_manifest_binds_schema_digest_without_loading_the_extension_component() {
        let source = SchemaSource::new("schemas/example.xsd", b"schema".to_vec());
        let manifest = ExtensionPackageManifest {
            namespace_uri: "urn:example".into(),
            package_version: "1.0.0".into(),
            extension_artifact: "components/example.wasm".into(),
            schema_artifact: source.system_id.clone(),
            schema_sha256: hex::encode(Sha256::digest(&source.bytes)),
            wit_world: "stateforward:scxml/executable-content-extension".into(),
            wit_version: "0.2.0".into(),
            env_slots: Vec::new(),
            http_origins: Vec::new(),
        };
        assert_eq!(manifest.verify_schema(&source), Ok(()));
        let mut invalid = manifest.clone();
        invalid.schema_sha256 = "00".repeat(32);
        assert!(invalid.verify_schema(&source).is_err());
        assert_eq!(manifest.namespace_uri, "urn:example");
    }

    #[test]
    fn package_manifest_bounds_its_declared_slots_and_origins() {
        let source = SchemaSource::new("schemas/example.xsd", b"schema".to_vec());
        let manifest = ExtensionPackageManifest {
            namespace_uri: "urn:example".into(),
            package_version: "1.0.0".into(),
            extension_artifact: "components/example.wasm".into(),
            schema_artifact: source.system_id.clone(),
            schema_sha256: hex::encode(Sha256::digest(&source.bytes)),
            wit_world: SUPPORTED_NAMESPACE_WIT_WORLD.into(),
            wit_version: SUPPORTED_NAMESPACE_WIT_VERSION.into(),
            env_slots: vec!["apikey".into(), "base-url2".into()],
            http_origins: vec![
                "https://api.example".into(),
                "http://127.0.0.1:8080/v1".into(),
            ],
        };
        assert_eq!(manifest.verify_schema(&source), Ok(()));
        for slots in [
            vec!["ApiKey".to_owned()],
            vec!["1key".to_owned()],
            vec!["keyexpr".to_owned()],
            vec!["key".to_owned(), "key".to_owned()],
            vec!["k".repeat(MAX_ENV_SLOT_BYTES + 1)],
            (0..=MAX_EXTENSION_PACKAGE_LIST_ITEMS)
                .map(|index| format!("s{index}"))
                .collect(),
        ] {
            let mut invalid = manifest.clone();
            invalid.env_slots = slots;
            assert!(
                invalid.verify_schema(&source).is_err(),
                "{:?}",
                invalid.env_slots
            );
        }
        for origin in [
            "ftp://a.example",
            "https://",
            "https:///path",
            "https://user@a.example",
            "https://a.example/?q",
            "https://a.example/#f",
            "https://a .example",
            "a.example",
        ] {
            let mut invalid = manifest.clone();
            invalid.http_origins = vec![origin.into()];
            assert!(invalid.verify_schema(&source).is_err(), "{origin}");
        }
    }

    #[test]
    fn package_manifest_rejects_unsupported_wit_identity_exactly() {
        let source = SchemaSource::new("schemas/example.xsd", b"schema".to_vec());
        let manifest = ExtensionPackageManifest {
            namespace_uri: "urn:example".into(),
            package_version: "1.0.0".into(),
            extension_artifact: "components/example.wasm".into(),
            schema_artifact: source.system_id.clone(),
            schema_sha256: hex::encode(Sha256::digest(&source.bytes)),
            wit_world: "stateforward:scxml/other".into(),
            wit_version: SUPPORTED_NAMESPACE_WIT_VERSION.into(),
            env_slots: Vec::new(),
            http_origins: Vec::new(),
        };
        assert_eq!(
            manifest.verify_schema(&source),
            Err(format!(
                "extension package WIT world must equal `{SUPPORTED_NAMESPACE_WIT_WORLD}`"
            ))
        );

        let mut wrong_version = manifest;
        wrong_version.wit_world = SUPPORTED_NAMESPACE_WIT_WORLD.into();
        wrong_version.wit_version = "0.3.0".into();
        assert_eq!(
            wrong_version.verify_schema(&source),
            Err(format!(
                "extension package WIT version must equal `{SUPPORTED_NAMESPACE_WIT_VERSION}`"
            ))
        );
    }

    #[test]
    fn supported_namespace_wit_identity_matches_canonical_wit() {
        let wit = include_str!("../../scxml-wit/wit/scxml/scxml.wit");
        let package = format!("package stateforward:scxml@{SUPPORTED_NAMESPACE_WIT_VERSION};");
        assert!(
            wit.lines().any(|line| line == package),
            "canonical WIT must declare {package}"
        );
        assert!(wit.contains("world executable-content-extension {"));
        assert_eq!(
            SUPPORTED_NAMESPACE_WIT_WORLD,
            "stateforward:scxml/executable-content-extension"
        );
    }

    #[test]
    fn package_manifest_bounds_schema_before_digesting() {
        let manifest = ExtensionPackageManifest {
            namespace_uri: "urn:example".into(),
            package_version: "1.0.0".into(),
            extension_artifact: "components/example.wasm".into(),
            schema_artifact: "schemas/example.xsd".into(),
            schema_sha256: "00".repeat(32),
            wit_world: SUPPORTED_NAMESPACE_WIT_WORLD.into(),
            wit_version: SUPPORTED_NAMESPACE_WIT_VERSION.into(),
            env_slots: Vec::new(),
            http_origins: Vec::new(),
        };
        let oversized_system_id = SchemaSource::new(
            "x".repeat(MAX_COMPONENT_METADATA_BYTES + 1),
            b"schema".to_vec(),
        );
        assert_eq!(
            manifest.verify_schema(&oversized_system_id),
            Err(format!(
                "schema system-id exceeds {MAX_COMPONENT_METADATA_BYTES} bytes"
            ))
        );

        let oversized_bytes = SchemaSource::new(
            "schemas/example.xsd",
            vec![0; MAX_COMPONENT_SCHEMA_BYTES + 1],
        );
        assert_eq!(
            manifest.verify_schema(&oversized_bytes),
            Err(format!(
                "schema bytes exceed {MAX_COMPONENT_SCHEMA_BYTES} bytes"
            ))
        );
    }

    #[test]
    fn package_manifest_bounds_fields_as_a_pure_check_without_telemetry() {
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let source = SchemaSource::new("schemas/example.xsd", b"schema".to_vec());
        let manifest = ExtensionPackageManifest {
            namespace_uri: "x".repeat(MAX_EXTENSION_PACKAGE_FIELD_BYTES + 1),
            package_version: "1.0.0".into(),
            extension_artifact: "components/example.wasm".into(),
            schema_artifact: source.system_id.clone(),
            schema_sha256: hex::encode(Sha256::digest(&source.bytes)),
            wit_world: SUPPORTED_NAMESPACE_WIT_WORLD.into(),
            wit_version: SUPPORTED_NAMESPACE_WIT_VERSION.into(),
            env_slots: Vec::new(),
            http_origins: Vec::new(),
        };
        assert_eq!(
            manifest.verify_schema(&source),
            Err(format!(
                "extension package field `namespace-uri` exceeds {MAX_EXTENSION_PACKAGE_FIELD_BYTES} bytes"
            ))
        );
        // Manifest verification is a pure predicate that runs inside its
        // caller's boundary span; it records nothing of its own.
        assert!(recorder.observations().is_empty());
    }
}
