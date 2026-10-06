//! The WASI interfaces an invoked-service component imports, and the
//! runtime resources the embedder grants it.
//!
//! Nothing WASI is ambient, and nothing is declared in a package manifest:
//! what a service component may use is what it imports.
//!
//! * **Imports** — [`WasiPlan::new`] reads the component's own `wasi:*`
//!   imports (its world's `wasi:clocks/monotonic-clock@0.3.0`, which the host
//!   runs on the session's logical clock, is linked by the service world and
//!   is not part of the plan). Clocks (`wasi:clocks/wall-clock`,
//!   `wasi:clocks/monotonic-clock`), randomness (`wasi:random/*`), and the
//!   `wasi:io` interfaces need no grant. Every other `wasi:*` interface,
//!   such as `wasi:cli` or `wasi:http`, is never linked: a component that
//!   imports one is refused ([`ArtifactError::UnknownImport`]).
//! * **Resources** — only runtime resources need a grant, named after
//!   wasmtime CLI's options ([`WasiOptions`]): `wasi:filesystem` needs at
//!   least one preopened directory (`--dir HOST::GUEST`), `wasi:sockets/tcp`
//!   network access (`-S inherit-network`: any address), and
//!   `wasi:sockets/ip-name-lookup` `-S allow-ip-name-lookup`. A component
//!   importing one without its grant is refused
//!   ([`ArtifactError::WasiResourceUngranted`]), and so is a grant whose
//!   interface the component does not import
//!   ([`ArtifactError::WasiGrantUndeclared`]). The session host plans every
//!   binding when it is bound, before any session starts.
//! * **Linking** — [`WasiPlan::add_to_linker`] adds exactly the planned
//!   interfaces from `wasmtime-wasi`'s WASI 0.2 synchronous bindings (the
//!   interfaces a `wasm32-wasip2` guest imports), once each. `wasi:cli`,
//!   `wasi:http`, and UDP are never linked.
//! * **Context** — [`WasiPlan::context`] builds one invocation's context:
//!   each granted directory preopened at its guest path, read-write (as
//!   wasmtime's `--dir`), TCP connections to any address only under
//!   `inherit-network` (UDP is refused), name lookup only when granted. Clocks and randomness are the host's.
//!
//! Telemetry: no span of its own. Planning failures surface through the
//! binding's error, and WASI calls run inside the invocation's spans.

use std::path::PathBuf;

use wasmtime::component::{HasData, Linker, ResourceTable};
use wasmtime_wasi::sockets::SocketAddrUse;
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiView};

use crate::{ArtifactError, HostError};

/// Maximum preopened directories and TCP addresses of one [`WasiOptions`].
const MAX_WASI_OPTION_ITEMS: usize = 32;
/// Maximum bytes of one guest path.
const MAX_WASI_GUEST_PATH_BYTES: usize = 1_024;

/// The runtime resources the embedder grants one invoked service's
/// component, named after wasmtime CLI's options.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WasiOptions {
    /// `--dir HOST::GUEST`: each host directory, preopened read-write at
    /// its absolute guest path (at most 32, each guest path once).
    pub dirs: Vec<(PathBuf, String)>,
    /// `-S inherit-network`: TCP connections may reach any address;
    /// without it, TCP is denied.
    pub inherit_network: bool,
    /// `-S allow-ip-name-lookup`.
    pub allow_ip_name_lookup: bool,
}

impl WasiOptions {
    /// Whether it grants nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty() && !self.inherit_network && !self.allow_ip_name_lookup
    }

    /// Whether every entry is within its bounds: guest paths absolute,
    /// distinct, and bounded, no empty host path, and at most 32
    /// directories.
    pub(crate) fn is_valid(&self) -> bool {
        self.dirs.len() <= MAX_WASI_OPTION_ITEMS
            && self.dirs.iter().enumerate().all(|(index, (host, guest))| {
                !host.as_os_str().is_empty()
                    && guest.starts_with('/')
                    && guest.len() <= MAX_WASI_GUEST_PATH_BYTES
                    && !guest.contains('\0')
                    && self
                        .dirs
                        .get(..index)
                        .is_some_and(|before| before.iter().all(|(_, seen)| seen != guest))
            })
    }
}

/// One link unit: a WASI interface `wasmtime-wasi` provides as a whole.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Unit {
    IoError,
    IoPoll,
    IoStreams,
    WallClock,
    MonotonicClock,
    Random,
    RandomInsecure,
    RandomInsecureSeed,
    FilesystemTypes,
    FilesystemPreopens,
    Network,
    InstanceNetwork,
    TcpCreateSocket,
    Tcp,
    IpNameLookup,
}

/// The resource a unit needs granted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Resource {
    Dirs,
    Tcp,
    IpNameLookup,
}

impl Unit {
    const ALL: [Self; 15] = [
        Self::IoError,
        Self::IoPoll,
        Self::IoStreams,
        Self::WallClock,
        Self::MonotonicClock,
        Self::Random,
        Self::RandomInsecure,
        Self::RandomInsecureSeed,
        Self::FilesystemTypes,
        Self::FilesystemPreopens,
        Self::Network,
        Self::InstanceNetwork,
        Self::TcpCreateSocket,
        Self::Tcp,
        Self::IpNameLookup,
    ];

    /// The unversioned interface name a component imports.
    const fn name(self) -> &'static str {
        match self {
            Self::IoError => "wasi:io/error",
            Self::IoPoll => "wasi:io/poll",
            Self::IoStreams => "wasi:io/streams",
            Self::WallClock => "wasi:clocks/wall-clock",
            Self::MonotonicClock => "wasi:clocks/monotonic-clock",
            Self::Random => "wasi:random/random",
            Self::RandomInsecure => "wasi:random/insecure",
            Self::RandomInsecureSeed => "wasi:random/insecure-seed",
            Self::FilesystemTypes => "wasi:filesystem/types",
            Self::FilesystemPreopens => "wasi:filesystem/preopens",
            Self::Network => "wasi:sockets/network",
            Self::InstanceNetwork => "wasi:sockets/instance-network",
            Self::TcpCreateSocket => "wasi:sockets/tcp-create-socket",
            Self::Tcp => "wasi:sockets/tcp",
            Self::IpNameLookup => "wasi:sockets/ip-name-lookup",
        }
    }

    /// The resource it needs granted, if any.
    const fn resource(self) -> Option<Resource> {
        match self {
            Self::FilesystemTypes | Self::FilesystemPreopens => Some(Resource::Dirs),
            Self::TcpCreateSocket | Self::Tcp => Some(Resource::Tcp),
            Self::IpNameLookup => Some(Resource::IpNameLookup),
            _ => None,
        }
    }
}

/// Whether `import` is the invoked-service world's own logical-clock
/// import (`wasi:clocks/monotonic-clock@0.3.0` and the `wasi:clocks/types`
/// it uses), which the service linker provides outside the plan.
pub(crate) fn is_world_clock(import: &str) -> bool {
    matches!(
        import,
        "wasi:clocks/monotonic-clock@0.3.0" | "wasi:clocks/types@0.3.0"
    )
}

/// The WASI interfaces one bound service's component imports, and what the
/// run grants them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WasiPlan {
    /// Every unit the component imports, sorted and distinct.
    linked: Vec<Unit>,
    /// The granted resources.
    options: WasiOptions,
}

/// The `wasi:io` host state is the instance's resource table.
struct IoTable;

impl HasData for IoTable {
    type Data<'a> = &'a mut ResourceTable;
}

/// One invocation's WASI state: its context and its resource table.
pub(crate) struct WasiState {
    ctx: WasiCtx,
    table: ResourceTable,
}

impl std::fmt::Debug for WasiState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WasiState").finish_non_exhaustive()
    }
}

impl WasiState {
    /// The `wasi-view` of this state.
    pub(crate) fn view(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

impl WasiPlan {
    /// Plans the WASI interfaces in `imports` (a component's top-level
    /// import names) with `options`.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::WasiGrantInvalid`] for options outside their bounds,
    /// [`ArtifactError::UnknownImport`] for a `wasi:*` import this
    /// host never links, [`ArtifactError::WasiResourceUngranted`] for an
    /// imported interface whose resource `options` does not grant, and
    /// [`ArtifactError::WasiGrantUndeclared`] for a granted resource no
    /// import uses.
    pub(crate) fn new(imports: &[String], options: &WasiOptions) -> Result<Self, HostError> {
        if !options.is_valid() {
            return Err(ArtifactError::WasiGrantInvalid.into());
        }
        let mut linked = Vec::new();
        for import in imports {
            if !import.starts_with("wasi:") || is_world_clock(import) {
                continue;
            }
            let interface = import
                .split_once('@')
                .map_or(import.as_str(), |(interface, _)| interface);
            let unit = Unit::ALL
                .into_iter()
                .find(|unit| unit.name() == interface)
                .ok_or(ArtifactError::UnknownImport)?;
            linked.push(unit);
        }
        linked.sort_unstable();
        linked.dedup();
        let uses = |resource: Resource| linked.iter().any(|unit| unit.resource() == Some(resource));
        for (resource, granted) in [
            (Resource::Dirs, !options.dirs.is_empty()),
            (Resource::Tcp, options.inherit_network),
            (Resource::IpNameLookup, options.allow_ip_name_lookup),
        ] {
            match (uses(resource), granted) {
                (true, false) => return Err(ArtifactError::WasiResourceUngranted.into()),
                (false, true) => return Err(ArtifactError::WasiGrantUndeclared.into()),
                _ => {}
            }
        }
        Ok(Self {
            linked,
            options: options.clone(),
        })
    }

    /// Adds exactly the planned interfaces to `linker`, each once.
    ///
    /// # Errors
    ///
    /// A Wasmtime linker failure.
    pub(crate) fn add_to_linker<T: WasiView + 'static>(
        &self,
        linker: &mut Linker<T>,
    ) -> Result<(), HostError> {
        use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView};
        use wasmtime_wasi::filesystem::{WasiFilesystem, WasiFilesystemView};
        use wasmtime_wasi::p2::bindings::{clocks, filesystem, io, random, sockets, sync};
        use wasmtime_wasi::random::{WasiRandom, WasiRandomView};
        use wasmtime_wasi::sockets::{WasiSockets, WasiSocketsView};

        fn table<T: WasiView>(state: &mut T) -> &mut ResourceTable {
            state.ctx().table
        }
        let options = sockets::network::LinkOptions::default();
        for unit in &self.linked {
            let l = &mut *linker;
            match unit {
                Unit::IoError => io::error::add_to_linker::<T, IoTable>(l, table)?,
                Unit::IoPoll => sync::io::poll::add_to_linker::<T, IoTable>(l, table)?,
                Unit::IoStreams => sync::io::streams::add_to_linker::<T, IoTable>(l, table)?,
                Unit::WallClock => {
                    clocks::wall_clock::add_to_linker::<T, WasiClocks>(l, T::clocks)?;
                }
                Unit::MonotonicClock => {
                    clocks::monotonic_clock::add_to_linker::<T, WasiClocks>(l, T::clocks)?;
                }
                Unit::Random => random::random::add_to_linker::<T, WasiRandom>(l, T::random)?,
                Unit::RandomInsecure => {
                    random::insecure::add_to_linker::<T, WasiRandom>(l, T::random)?;
                }
                Unit::RandomInsecureSeed => {
                    random::insecure_seed::add_to_linker::<T, WasiRandom>(l, T::random)?;
                }
                Unit::FilesystemTypes => {
                    sync::filesystem::types::add_to_linker::<T, WasiFilesystem>(l, T::filesystem)?;
                }
                Unit::FilesystemPreopens => {
                    filesystem::preopens::add_to_linker::<T, WasiFilesystem>(l, T::filesystem)?;
                }
                Unit::Network => {
                    sockets::network::add_to_linker::<T, WasiSockets>(l, &options, T::sockets)?;
                }
                Unit::InstanceNetwork => {
                    sockets::instance_network::add_to_linker::<T, WasiSockets>(l, T::sockets)?;
                }
                Unit::TcpCreateSocket => {
                    sockets::tcp_create_socket::add_to_linker::<T, WasiSockets>(l, T::sockets)?;
                }
                Unit::Tcp => sync::sockets::tcp::add_to_linker::<T, WasiSockets>(l, T::sockets)?,
                Unit::IpNameLookup => {
                    sync::sockets::ip_name_lookup::add_to_linker::<T, WasiSockets>(l, T::sockets)?;
                }
            }
        }
        Ok(())
    }

    /// One invocation's WASI state: every granted directory preopened
    /// read-write at its guest path, TCP to any address only under
    /// `inherit-network`, name lookup only when granted.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::WasiGrantInvalid`] when a granted directory cannot be
    /// opened.
    pub(crate) fn context(&self) -> Result<WasiState, HostError> {
        let mut builder = WasiCtxBuilder::new();
        for (host, guest) in &self.options.dirs {
            builder
                .preopened_dir(host, guest, FsPerms::ReadWrite)
                .map_err(|_| HostError::from(ArtifactError::WasiGrantInvalid))?;
        }
        let network = self.options.inherit_network;
        builder
            .allow_tcp(network)
            .allow_udp(false)
            .allow_ip_name_lookup(self.options.allow_ip_name_lookup)
            .socket_addr_check(move |_, usage| {
                let admitted =
                    network && matches!(usage, SocketAddrUse::TcpConnect | SocketAddrUse::TcpBind);
                Box::pin(async move { admitted })
            });
        Ok(WasiState {
            ctx: builder.build(),
            table: ResourceTable::new(),
        })
    }
}

/// The top-level import names of one component, read without compiling it.
///
/// # Errors
///
/// [`HostError::Component`] `component-imports-unreadable` for bytes that
/// are not a component.
pub(crate) fn component_imports(bytes: &[u8]) -> Result<Vec<String>, HostError> {
    let unreadable = || HostError::Component("component-imports-unreadable".into());
    let mut depth = 0_usize;
    let mut imports = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        match payload.map_err(|_| unreadable())? {
            wasmparser::Payload::ModuleSection { .. }
            | wasmparser::Payload::ComponentSection { .. } => {
                depth = depth.saturating_add(1);
            }
            wasmparser::Payload::End(_) => depth = depth.saturating_sub(1),
            wasmparser::Payload::ComponentImportSection(reader) if depth == 0 => {
                for import in reader {
                    imports.push(import.map_err(|_| unreadable())?.name.name.to_owned());
                }
            }
            _ => {}
        }
    }
    Ok(imports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(imports: &[&str], options: &WasiOptions) -> ArtifactError {
        let imports: Vec<String> = imports.iter().map(|import| (*import).to_owned()).collect();
        match WasiPlan::new(&imports, options) {
            Err(HostError::Artifact(error)) => error,
            Err(other) => panic!("unexpected failure: {other}"),
            Ok(_) => panic!("the plan must be refused"),
        }
    }

    fn plan(imports: &[&str], options: &WasiOptions) -> WasiPlan {
        let imports: Vec<String> = imports.iter().map(|import| (*import).to_owned()).collect();
        WasiPlan::new(&imports, options).expect("planned")
    }

    fn dir() -> WasiOptions {
        WasiOptions {
            dirs: vec![(std::env::temp_dir(), "/workspace".into())],
            ..WasiOptions::default()
        }
    }

    #[test]
    fn the_world_clock_and_non_wasi_imports_plan_nothing() {
        let planned = plan(
            &[
                "wasi:clocks/monotonic-clock@0.3.0",
                "wasi:clocks/types@0.3.0",
                "stateforward:scxml/outbound-http@0.2.0",
            ],
            &WasiOptions::default(),
        );
        assert!(planned.linked.is_empty());
    }

    #[test]
    fn clocks_and_randomness_need_no_grant_and_cli_is_never_linked() {
        let planned = plan(
            &["wasi:clocks/wall-clock@0.2.9", "wasi:random/random@0.2.9"],
            &WasiOptions::default(),
        );
        assert!(!planned.linked.is_empty());
        assert_eq!(
            refusal(&["wasi:cli/environment@0.2.9"], &WasiOptions::default()),
            ArtifactError::UnknownImport
        );
        assert_eq!(
            refusal(
                &["wasi:http/outgoing-handler@0.2.9"],
                &WasiOptions::default()
            ),
            ArtifactError::UnknownImport
        );
    }

    #[test]
    fn runtime_resources_need_exactly_their_grant() {
        let filesystem = [
            "wasi:filesystem/types@0.2.9",
            "wasi:filesystem/preopens@0.2.9",
        ];
        assert_eq!(
            refusal(&filesystem, &WasiOptions::default()),
            ArtifactError::WasiResourceUngranted
        );
        assert!(plan(&filesystem, &dir()).context().is_ok());
        assert_eq!(
            refusal(&["wasi:random/random@0.2.9"], &dir()),
            ArtifactError::WasiGrantUndeclared
        );
        let tcp = WasiOptions {
            inherit_network: true,
            ..WasiOptions::default()
        };
        assert_eq!(
            refusal(&["wasi:sockets/tcp@0.2.9"], &WasiOptions::default()),
            ArtifactError::WasiResourceUngranted
        );
        assert!(plan(&["wasi:sockets/tcp@0.2.9"], &tcp).context().is_ok());
        let lookup = WasiOptions {
            allow_ip_name_lookup: true,
            ..WasiOptions::default()
        };
        assert_eq!(
            refusal(&["wasi:random/random@0.2.9"], &lookup),
            ArtifactError::WasiGrantUndeclared
        );
        assert!(
            !plan(&["wasi:sockets/ip-name-lookup@0.2.9"], &lookup)
                .linked
                .is_empty()
        );
    }

    #[test]
    fn options_are_bounded_and_guest_paths_distinct() {
        let mut twice = dir();
        twice.dirs.push((std::env::temp_dir(), "/workspace".into()));
        assert!(!twice.is_valid());
        let relative = WasiOptions {
            dirs: vec![(std::env::temp_dir(), "workspace".into())],
            ..WasiOptions::default()
        };
        assert!(!relative.is_valid());
        let many = WasiOptions {
            dirs: (0..=MAX_WASI_OPTION_ITEMS)
                .map(|index| (std::env::temp_dir(), format!("/d{index}")))
                .collect(),
            ..WasiOptions::default()
        };
        assert!(!many.is_valid());
        assert_eq!(
            refusal(&["wasi:filesystem/preopens@0.2.9"], &many),
            ArtifactError::WasiGrantInvalid
        );
    }

    #[test]
    fn a_missing_directory_fails_the_context_closed() {
        let missing = WasiOptions {
            dirs: vec![(
                std::env::temp_dir().join("scxml-host-wasi-no-such-directory"),
                "/workspace".into(),
            )],
            ..WasiOptions::default()
        };
        let planned = plan(&["wasi:filesystem/preopens@0.2.9"], &missing);
        assert!(matches!(
            planned.context(),
            Err(HostError::Artifact(ArtifactError::WasiGrantInvalid))
        ));
    }
}
