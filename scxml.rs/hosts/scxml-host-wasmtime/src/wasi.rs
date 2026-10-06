//! Declared and granted WASI permissions of invoked-service components.
//!
//! Nothing WASI is ambient. A service package may declare WASI 0.2
//! interfaces (`wasi-permissions`, [`scxml_types::wasi`]); the embedder
//! grants each of them for this run ([`WasiGrants`]); and the host links
//! into that component's linker exactly the declared **and** granted
//! interfaces, with the grant's value (a directory, a connect allowlist):
//!
//! * **Plan** — [`WasiPlan::new`] pairs every declared permission with its
//!   grant: a declared permission without a grant
//!   ([`ArtifactError::WasiPermissionUngranted`]) and a grant the package does
//!   not declare ([`ArtifactError::WasiGrantUndeclared`]) both fail the binding
//!   closed, before any session starts.
//! * **Imports** — [`WasiPlan::check_imports`] refuses a component that
//!   imports a `wasi:` interface the plan does not link
//!   ([`ArtifactError::WasiImportUndeclared`]); the only exception is an
//!   interface a linked one names for its types alone (`wasi:filesystem`
//!   uses `wasi:clocks/wall-clock.datetime`, `wasi:sockets/tcp` uses
//!   `wasi:clocks/monotonic-clock.duration`), which the host links as an
//!   empty instance, so the component can call nothing through it. The
//!   session host runs this check when it loads the service at session start
//!   (before the first macrostep) and again before every instantiation.
//! * **Linking** — [`WasiPlan::add_to_linker`] adds the planned interfaces
//!   (and the `wasi:io` interfaces they need) from `wasmtime-wasi`'s
//!   synchronous bindings, once each; nothing else. `wasi:cli`, `wasi:http`,
//!   and UDP are never linked.
//! * **Context** — [`WasiPlan::context`] builds one instance's context:
//!   each granted directory preopened at `/<mount>` with the declared
//!   access, TCP connections admitted only to the granted addresses (and the
//!   implicit wildcard bind a connect performs; listening, accepting, and UDP
//!   are refused), name lookup only when granted. Clocks and randomness are
//!   the host's.
//!
//! Telemetry: no span of its own. Planning failures surface through the
//! binding's error, import and link failures through the enclosing
//! `service-load` or `service-start` span, and WASI calls run inside the
//! service's export-call spans.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use scxml_types::{FilesystemAccess, WasiInterface, WasiPermission};
use wasmtime::component::{HasData, Linker, ResourceTable};
use wasmtime_wasi::sockets::SocketAddrUse;
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiView};

use crate::{ArtifactError, HostError};

/// Maximum addresses one `wasi:sockets/tcp` grant admits.
pub const MAX_WASI_TCP_ADDRESSES: usize = 32;
/// Maximum package URIs one [`WasiGrants`] grants to.
pub const MAX_WASI_GRANTED_PACKAGES: usize = 64;

/// The WASI package version of the interfaces this host links
/// (`wasmtime-wasi`'s; a component importing an older 0.2 version is linked
/// by semver compatibility).
const WASI_VERSION: &str = "0.2.12";

/// One grant the embedder gives one package for one declared permission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WasiGrant {
    /// `wasi:clocks`.
    Clocks,
    /// `wasi:filesystem` for one declared mount: the host directory the
    /// component sees at `/<mount>`.
    Filesystem {
        /// The declared mount name.
        mount: String,
        /// The host directory behind it.
        directory: PathBuf,
    },
    /// `wasi:random`.
    Random,
    /// `wasi:sockets/ip-name-lookup`.
    SocketsIpNameLookup,
    /// `wasi:sockets/tcp`: the remote addresses the component may connect
    /// to (at most [`MAX_WASI_TCP_ADDRESSES`], at least one).
    SocketsTcp {
        /// The admitted remote addresses.
        connect: Vec<SocketAddr>,
    },
}

impl WasiGrant {
    /// The interface it grants.
    #[must_use]
    pub const fn interface(&self) -> WasiInterface {
        match self {
            Self::Clocks => WasiInterface::Clocks,
            Self::Filesystem { .. } => WasiInterface::Filesystem,
            Self::Random => WasiInterface::Random,
            Self::SocketsIpNameLookup => WasiInterface::SocketsIpNameLookup,
            Self::SocketsTcp { .. } => WasiInterface::SocketsTcp,
        }
    }

    /// The mount name, for a filesystem grant.
    #[must_use]
    pub fn mount(&self) -> Option<&str> {
        match self {
            Self::Filesystem { mount, .. } => Some(mount),
            _ => None,
        }
    }

    fn key(&self) -> (WasiInterface, Option<&str>) {
        (self.interface(), self.mount())
    }

    fn is_valid(&self) -> bool {
        match self {
            Self::Filesystem { mount, directory } => {
                scxml_types::is_wasi_mount(mount) && !directory.as_os_str().is_empty()
            }
            Self::SocketsTcp { connect } => {
                !connect.is_empty() && connect.len() <= MAX_WASI_TCP_ADDRESSES
            }
            Self::Clocks | Self::Random | Self::SocketsIpNameLookup => true,
        }
    }
}

/// The run's WASI grants, by exact package URI (a service type URI).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WasiGrants {
    grants: BTreeMap<String, Vec<WasiGrant>>,
}

impl WasiGrants {
    /// No grant: a package that declares a WASI permission fails its binding.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Grants `grant` to the package bound to exactly `uri`.
    ///
    /// # Errors
    ///
    /// [`HostError::Artifact`] [`ArtifactError::WasiGrantInvalid`]: an
    /// invalid mount name or empty directory, a TCP grant with no address
    /// or more than [`MAX_WASI_TCP_ADDRESSES`], the same interface (or
    /// mount) granted twice to one URI, or more than
    /// [`MAX_WASI_GRANTED_PACKAGES`] URIs.
    pub fn grant(mut self, uri: &str, grant: WasiGrant) -> Result<Self, HostError> {
        let invalid = || HostError::from(ArtifactError::WasiGrantInvalid);
        if !grant.is_valid()
            || (!self.grants.contains_key(uri) && self.grants.len() >= MAX_WASI_GRANTED_PACKAGES)
        {
            return Err(invalid());
        }
        let entries = self.grants.entry(uri.to_owned()).or_default();
        if entries.len() >= scxml_types::MAX_WASI_PERMISSIONS
            || entries.iter().any(|granted| granted.key() == grant.key())
        {
            return Err(invalid());
        }
        entries.push(grant);
        Ok(self)
    }

    /// The grants to exactly `uri`.
    #[must_use]
    pub fn for_uri(&self, uri: &str) -> &[WasiGrant] {
        self.grants.get(uri).map_or(&[], Vec::as_slice)
    }

    /// Every URI granted to, in order.
    pub fn uris(&self) -> impl Iterator<Item = &str> {
        self.grants.keys().map(String::as_str)
    }

    /// Whether nothing is granted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
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

impl Unit {
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

    /// The units one interface links, and the units it names for types
    /// only.
    const fn of(interface: WasiInterface) -> (&'static [Self], &'static [Self]) {
        match interface {
            WasiInterface::Clocks => (&[Self::WallClock, Self::MonotonicClock, Self::IoPoll], &[]),
            WasiInterface::Random => (
                &[Self::Random, Self::RandomInsecure, Self::RandomInsecureSeed],
                &[],
            ),
            WasiInterface::Filesystem => (
                &[
                    Self::FilesystemTypes,
                    Self::FilesystemPreopens,
                    Self::IoError,
                    Self::IoPoll,
                    Self::IoStreams,
                ],
                &[Self::WallClock],
            ),
            WasiInterface::SocketsTcp => (
                &[
                    Self::Network,
                    Self::InstanceNetwork,
                    Self::TcpCreateSocket,
                    Self::Tcp,
                    Self::IoError,
                    Self::IoPoll,
                    Self::IoStreams,
                ],
                &[Self::MonotonicClock],
            ),
            WasiInterface::SocketsIpNameLookup => (
                &[
                    Self::Network,
                    Self::InstanceNetwork,
                    Self::IpNameLookup,
                    Self::IoPoll,
                ],
                &[],
            ),
        }
    }
}

/// The declared-and-granted WASI permissions of one bound package.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WasiPlan {
    /// Every declared permission with its grant, in canonical order.
    entries: Vec<(WasiPermission, WasiGrant)>,
    /// Every link unit, sorted and distinct.
    linked: Vec<Unit>,
    /// The units named for types only that no grant links, sorted and
    /// distinct.
    carried: Vec<Unit>,
}

/// The `wasi:io` host state is the instance's resource table.
struct IoTable;

impl HasData for IoTable {
    type Data<'a> = &'a mut ResourceTable;
}

/// One instance's WASI state: its context and its resource table.
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
    /// Pairs every permission in `declared` with exactly one grant in
    /// `granted`.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::WasiPermissionsInvalid`] for a non-canonical
    /// declared list, [`ArtifactError::WasiPermissionUngranted`] for a
    /// declared permission no grant covers, and
    /// [`ArtifactError::WasiGrantUndeclared`] for a grant no declared
    /// permission names.
    pub(crate) fn new(
        declared: &[WasiPermission],
        granted: &[WasiGrant],
    ) -> Result<Self, HostError> {
        scxml_types::check_wasi_permissions(declared)
            .map_err(|_| HostError::from(ArtifactError::WasiPermissionsInvalid))?;
        if granted.iter().any(|grant| {
            !declared
                .iter()
                .any(|permission| permission.key() == grant.key())
        }) {
            return Err(ArtifactError::WasiGrantUndeclared.into());
        }
        let mut entries = Vec::with_capacity(declared.len());
        let mut linked = Vec::new();
        let mut carried = Vec::new();
        for permission in declared {
            let grant = granted
                .iter()
                .find(|grant| grant.key() == permission.key())
                .ok_or(ArtifactError::WasiPermissionUngranted)?;
            entries.push((permission.clone(), grant.clone()));
            let (units, types) = Unit::of(permission.interface());
            linked.extend_from_slice(units);
            carried.extend_from_slice(types);
        }
        linked.sort_unstable();
        linked.dedup();
        carried.sort_unstable();
        carried.dedup();
        carried.retain(|unit| linked.binary_search(unit).is_err());
        Ok(Self {
            entries,
            linked,
            carried,
        })
    }

    /// Refuses `component` when it imports a `wasi:` interface this plan
    /// does not link, unless the plan names it for its types alone and the
    /// component imports no function or resource from it. Imports outside
    /// `wasi:` are the component world's, checked by the linker.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::WasiImportUndeclared`].
    pub(crate) fn check_imports(
        &self,
        engine: &wasmtime::Engine,
        component: &wasmtime::component::Component,
    ) -> Result<(), HostError> {
        use wasmtime::component::types::ComponentItem;

        let undeclared = || HostError::from(ArtifactError::WasiImportUndeclared);
        for (name, item) in component.component_type().imports(engine) {
            if !name.starts_with("wasi:") {
                continue;
            }
            let interface = name
                .split_once('@')
                .map_or(name, |(interface, _)| interface);
            if self.linked.iter().any(|unit| unit.name() == interface) {
                continue;
            }
            if !self.carried.iter().any(|unit| unit.name() == interface) {
                return Err(undeclared());
            }
            let ComponentItem::ComponentInstance(instance) = item.ty else {
                return Err(undeclared());
            };
            if instance.exports(engine).any(|(_, export)| {
                matches!(
                    export.ty,
                    ComponentItem::ComponentFunc(_)
                        | ComponentItem::CoreFunc(_)
                        | ComponentItem::Resource(_)
                        | ComponentItem::Component(_)
                        | ComponentItem::ComponentInstance(_)
                        | ComponentItem::Module(_)
                )
            }) {
                return Err(undeclared());
            }
        }
        Ok(())
    }

    /// Adds exactly the planned interfaces to `linker` (each once), and an
    /// empty instance for each interface named for types only.
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
        for unit in &self.carried {
            linker.instance(&format!("{}@{WASI_VERSION}", unit.name()))?;
        }
        Ok(())
    }

    /// One instance's WASI state: every granted directory preopened at
    /// `/<mount>` with its declared access, TCP connects admitted only to the
    /// granted addresses, name lookup only when granted.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::WasiGrantInvalid`] when a granted directory cannot be
    /// opened.
    pub(crate) fn context(&self) -> Result<WasiState, HostError> {
        let mut builder = WasiCtxBuilder::new();
        let mut connect: Vec<SocketAddr> = Vec::new();
        let mut lookup = false;
        for (permission, grant) in &self.entries {
            match (permission, grant) {
                (
                    WasiPermission::Filesystem { mount, access },
                    WasiGrant::Filesystem { directory, .. },
                ) => {
                    let perms = match access {
                        FilesystemAccess::ReadOnly => FsPerms::ReadOnly,
                        FilesystemAccess::ReadWrite => FsPerms::ReadWrite,
                    };
                    builder
                        .preopened_dir(directory, format!("/{mount}"), perms)
                        .map_err(|_| HostError::from(ArtifactError::WasiGrantInvalid))?;
                }
                (WasiPermission::SocketsTcp, WasiGrant::SocketsTcp { connect: granted }) => {
                    connect.extend_from_slice(granted);
                }
                (WasiPermission::SocketsIpNameLookup, WasiGrant::SocketsIpNameLookup) => {
                    lookup = true;
                }
                _ => {}
            }
        }
        let connect: Arc<[SocketAddr]> = Arc::from(connect);
        builder
            .allow_tcp(!connect.is_empty())
            .allow_udp(false)
            .allow_ip_name_lookup(lookup)
            .socket_addr_check(move |address, usage| {
                let admitted = match usage {
                    SocketAddrUse::TcpConnect => connect.contains(&address),
                    // The implicit bind a connect performs.
                    SocketAddrUse::TcpBind => {
                        !connect.is_empty() && address.ip().is_unspecified() && address.port() == 0
                    }
                    _ => false,
                };
                Box::pin(async move { admitted })
            });
        Ok(WasiState {
            ctx: builder.build(),
            table: ResourceTable::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(access: FilesystemAccess) -> WasiPermission {
        WasiPermission::Filesystem {
            mount: "workspace".into(),
            access,
        }
    }

    fn directory() -> WasiGrant {
        WasiGrant::Filesystem {
            mount: "workspace".into(),
            directory: std::env::temp_dir(),
        }
    }

    fn refusal(declared: &[WasiPermission], granted: &[WasiGrant]) -> ArtifactError {
        match WasiPlan::new(declared, granted) {
            Err(HostError::Artifact(error)) => error,
            Err(other) => panic!("unexpected failure: {other}"),
            Ok(_) => panic!("the plan must be refused"),
        }
    }

    #[test]
    fn nothing_declared_plans_nothing() {
        let plan = WasiPlan::new(&[], &[]).expect("an empty plan");
        assert_eq!(plan, WasiPlan::default());
    }

    #[test]
    fn every_declared_permission_needs_exactly_its_grant() {
        let declared = [workspace(FilesystemAccess::ReadWrite)];
        assert_eq!(
            refusal(&declared, &[]),
            ArtifactError::WasiPermissionUngranted
        );
        assert_eq!(
            refusal(&declared, &[directory(), WasiGrant::Random]),
            ArtifactError::WasiGrantUndeclared
        );
        let other_mount = WasiGrant::Filesystem {
            mount: "other".into(),
            directory: std::env::temp_dir(),
        };
        assert_eq!(
            refusal(&declared, &[other_mount]),
            ArtifactError::WasiGrantUndeclared
        );
        assert_eq!(
            refusal(&[WasiPermission::Random, WasiPermission::Clocks], &[]),
            ArtifactError::WasiPermissionsInvalid,
            "a non-canonical declaration is refused before any grant is read"
        );
    }

    #[test]
    fn a_filesystem_grant_links_its_interfaces_and_carries_the_clock_types() {
        let plan = WasiPlan::new(&[workspace(FilesystemAccess::ReadOnly)], &[directory()])
            .expect("granted");
        assert_eq!(
            plan.linked,
            [
                Unit::IoError,
                Unit::IoPoll,
                Unit::IoStreams,
                Unit::FilesystemTypes,
                Unit::FilesystemPreopens
            ]
        );
        assert_eq!(plan.carried, [Unit::WallClock]);
        // Granting the clocks too links the wall clock instead of carrying
        // its types.
        let plan = WasiPlan::new(
            &[
                WasiPermission::Clocks,
                workspace(FilesystemAccess::ReadOnly),
            ],
            &[directory(), WasiGrant::Clocks],
        )
        .expect("granted");
        assert!(plan.carried.is_empty());
        assert!(plan.linked.contains(&Unit::WallClock));
        assert!(plan.linked.contains(&Unit::MonotonicClock));
    }

    #[test]
    fn sockets_share_their_network_interfaces() {
        let plan = WasiPlan::new(
            &[
                WasiPermission::SocketsIpNameLookup,
                WasiPermission::SocketsTcp,
            ],
            &[
                WasiGrant::SocketsTcp {
                    connect: vec![SocketAddr::from(([127, 0, 0, 1], 80))],
                },
                WasiGrant::SocketsIpNameLookup,
            ],
        )
        .expect("granted");
        assert_eq!(
            plan.linked
                .iter()
                .filter(|unit| **unit == Unit::Network)
                .count(),
            1
        );
        assert_eq!(plan.carried, [Unit::MonotonicClock]);
        assert!(plan.context().is_ok());
    }

    #[test]
    fn grants_are_bounded_and_unique() {
        let address = SocketAddr::from(([127, 0, 0, 1], 80));
        let invalid = |result: Result<WasiGrants, HostError>| {
            matches!(
                result,
                Err(HostError::Artifact(ArtifactError::WasiGrantInvalid))
            )
        };
        assert!(invalid(WasiGrants::new().grant(
            "urn:x",
            WasiGrant::SocketsTcp {
                connect: Vec::new()
            }
        )));
        assert!(invalid(WasiGrants::new().grant(
            "urn:x",
            WasiGrant::SocketsTcp {
                connect: vec![address; MAX_WASI_TCP_ADDRESSES + 1]
            }
        )));
        assert!(invalid(WasiGrants::new().grant(
            "urn:x",
            WasiGrant::Filesystem {
                mount: "Bad".into(),
                directory: std::env::temp_dir()
            }
        )));
        assert!(invalid(
            WasiGrants::new()
                .grant("urn:x", WasiGrant::Random)
                .and_then(|grants| grants.grant("urn:x", WasiGrant::Random))
        ));
        let mut grants = WasiGrants::new();
        for index in 0..MAX_WASI_GRANTED_PACKAGES {
            grants = grants
                .grant(&format!("urn:x:{index}"), WasiGrant::Random)
                .expect("within the bound");
        }
        assert!(invalid(grants.grant("urn:x:last", WasiGrant::Random)));
    }

    #[test]
    fn a_missing_directory_fails_the_context_closed() {
        let plan = WasiPlan::new(
            &[workspace(FilesystemAccess::ReadWrite)],
            &[WasiGrant::Filesystem {
                mount: "workspace".into(),
                directory: std::env::temp_dir().join("scxml-host-wasi-no-such-directory"),
            }],
        )
        .expect("planned");
        assert!(matches!(
            plan.context(),
            Err(HostError::Artifact(ArtifactError::WasiGrantInvalid))
        ));
    }
}
