//! Shared fixtures for the Wasmtime host integration tests.

use scxml_compiler_core::{
    Binding, InitialIr, MachineIr, StateIr, StateKind, TransitionIr, TransitionType,
};

/// The two-state example machine (`ready --finish--> done`) that runtime-mode
/// lifecycle proofs feed to the generic machine through `ir-source`.
pub fn example_machine() -> MachineIr {
    let state = |document_order, id: &str, kind, transitions| StateIr {
        document_order,
        id: Some(id.into()),
        kind,
        parent: None,
        children: Vec::new(),
        initial: None,
        onentry: Vec::new(),
        onexit: Vec::new(),
        transitions,
        data: Vec::new(),
        invokes: Vec::new(),
        donedata: None,
    };
    MachineIr {
        version: scxml_compiler_core::MACHINE_IR_VERSION,
        name: Some("example".into()),
        data_model: None,
        binding: Binding::Early,
        initial: InitialIr {
            document_order: 0,
            targets: vec![0],
            actions: Vec::new(),
        },
        states: vec![
            state(1, "ready", StateKind::Atomic, vec![0]),
            state(3, "done", StateKind::Final, Vec::new()),
        ],
        transitions: vec![TransitionIr {
            document_order: 2,
            source: 0,
            targets: vec![1],
            events: vec!["finish".into()],
            cond: None,
            transition_type: TransitionType::External,
            actions: Vec::new(),
            event_schema: None,
        }],
        data: Vec::new(),
        scripts: Vec::new(),
        extension_namespaces: Vec::new(),
    }
}

/// Canonical IR bytes of [`example_machine`].
pub fn example_ir() -> Vec<u8> {
    example_machine()
        .to_json()
        .expect("the example machine is valid canonical IR")
}
