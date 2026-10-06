//! Ordering tests over a scripted fake [`Sessions`] implementation.
//!
//! Each scenario drives one scripted, external-client, or drain ordering
//! over a root session and fake invoked sessions and asserts the exact sequence of session
//! commands (and, where relevant, clock advances, budget checks, and
//! checkpoints).

use std::cell::RefCell;
use std::collections::BTreeMap;

use super::*;

/// Failure of the fake sessions.
#[derive(Clone, Debug, Eq, PartialEq)]
enum FakeError {
    /// The shared command budget is exhausted.
    Budget,
    /// A scripted input failed validation.
    InvalidInput,
    /// An executor violation.
    Violation(Violation),
}

impl From<Violation> for FakeError {
    fn from(violation: Violation) -> Self {
        Self::Violation(violation)
    }
}

/// Target of one fake delayed send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Target {
    Local,
    Parent,
    Child(&'static str),
    /// Dispatched by the processor when due: queued on the invoking session
    /// (a `BasicHTTP` loopback POST), or nowhere for the root.
    Posted,
}

/// One scripted reaction of a fake session to an event.
#[derive(Clone, Copy, Debug)]
enum Act {
    /// Schedule `event` to `target` `ms` after the session's clock.
    Delay(u64, Target, &'static str),
    /// Schedule a local `event` at an absolute deadline.
    At(u64, &'static str),
    /// Immediate send to an invoked session (`#_<invokeid>`).
    ToChild(&'static str, &'static str),
    /// Autoforward to an invoked session.
    Forward(&'static str, &'static str),
    /// Immediate send to the invoking session (`#_parent`).
    ToParent(&'static str),
    /// Invoke a session.
    Invoke(&'static str),
    /// Cancel an invoked session; it sends `on-cancel` reactions first.
    Cancel(&'static str),
    /// Set return data.
    Return(&'static str),
    /// Reach a top-level final state.
    Finish,
}

type Path = Vec<String>;

/// One fake session: a logical clock with pending sends and a status.
#[derive(Default)]
struct FakeSession {
    now: u64,
    sequence: u64,
    pending: Vec<(u64, u64, Target, &'static str)>,
    finished: bool,
    return_data: Option<String>,
}

impl FakeSession {
    fn schedule(&mut self, deadline: u64, target: Target, event: &'static str) {
        self.pending.push((deadline, self.sequence, target, event));
        self.sequence += 1;
    }
}

/// Scripted fake sessions. A session is named by its path joined with `/`,
/// `"root"` for the root.
struct Fake {
    reactions: BTreeMap<(&'static str, String), Vec<Act>>,
    sessions: BTreeMap<Path, FakeSession>,
    /// Each session's path, by key.
    keys: BTreeMap<SessionKey, Path>,
    log: RefCell<Vec<String>>,
    steps: u64,
    max_commands: u64,
    log_budget: bool,
    /// How many times the executor read a session's deadline.
    deadline_reads: std::cell::Cell<u64>,
}

type FakeInvocations = InvokedSessions<String, String, String>;

fn name(path: &[String]) -> String {
    if path.is_empty() {
        "root".to_owned()
    } else {
        path.join("/")
    }
}

impl Fake {
    fn new(reactions: &[(&'static str, &str, &[Act])]) -> Self {
        Self {
            reactions: reactions
                .iter()
                .map(|(session, event, acts)| ((*session, (*event).to_owned()), acts.to_vec()))
                .collect(),
            sessions: BTreeMap::from([(Vec::new(), FakeSession::default())]),
            keys: BTreeMap::from([(SessionKey::ROOT, Vec::new())]),
            log: RefCell::new(Vec::new()),
            steps: 0,
            max_commands: 1_000,
            log_budget: false,
            deadline_reads: std::cell::Cell::new(0),
        }
    }

    fn with_budget(mut self, max_commands: u64) -> Self {
        self.max_commands = max_commands;
        self.log_budget = true;
        self
    }

    fn session(&mut self, path: &[String]) -> &mut FakeSession {
        self.sessions
            .get_mut(path)
            .expect("fake commands address only existing sessions")
    }

    fn path(&self, key: SessionKey) -> Path {
        self.keys
            .get(&key)
            .cloned()
            .expect("the executor addresses only present sessions")
    }

    fn keyed(&mut self, key: SessionKey) -> &mut FakeSession {
        let path = self.path(key);
        self.session(&path)
    }

    /// One engine command: admit against the budget, charge, record, react.
    fn command(
        &mut self,
        invoked_sessions: &mut FakeInvocations,
        key: SessionKey,
        event: &str,
    ) -> Result<(), FakeError> {
        if self.steps >= self.max_commands {
            return Err(FakeError::Budget);
        }
        self.steps += 1;
        let path = self.path(key);
        let who = name(&path);
        self.log.borrow_mut().push(format!("{who}<-{event}"));
        let reaction = self
            .reactions
            .keys()
            .find(|(session, reaction)| *session == who && reaction == event)
            .cloned();
        let acts = reaction
            .and_then(|reaction| self.reactions.get(&reaction).cloned())
            .unwrap_or_default();
        for act in acts {
            self.apply(invoked_sessions, key, act)?;
        }
        Ok(())
    }

    fn apply(
        &mut self,
        invoked_sessions: &mut FakeInvocations,
        key: SessionKey,
        act: Act,
    ) -> Result<(), FakeError> {
        let path = self.path(key);
        let child_path = |id: &str| {
            let mut child = path.clone();
            child.push(id.to_owned());
            child
        };
        let child = |invoked_sessions: &FakeInvocations, id: &str| {
            invoked_sessions.child(key, &id.to_owned())
        };
        match act {
            Act::Delay(ms, target, event) => {
                let session = self.session(&path);
                let deadline = session.now + ms;
                session.schedule(deadline, target, event);
            }
            Act::At(deadline, event) => {
                self.session(&path).schedule(deadline, Target::Local, event);
            }
            Act::ToChild(id, event) | Act::Forward(id, event) => {
                if let Some(child) = child(invoked_sessions, id) {
                    invoked_sessions.push_external(child, event.to_owned());
                }
            }
            Act::ToParent(event) => {
                invoked_sessions.push_to_parent(key, Invoked::Event(event.to_owned()));
            }
            Act::Invoke(id) => {
                let started = invoked_sessions.register(key, id.to_owned())?;
                let now = self.session(&path).now;
                self.sessions.insert(
                    child_path(id),
                    FakeSession {
                        now,
                        ..FakeSession::default()
                    },
                );
                self.keys.insert(started, child_path(id));
                // A command on another session inside this one.
                invoked_sessions.touch(started);
                self.command(invoked_sessions, started, "start")?;
            }
            Act::Cancel(id) => {
                if let Some(target) = child(invoked_sessions, id) {
                    invoked_sessions.begin_cancel(target);
                    invoked_sessions.touch(target);
                    self.command(invoked_sessions, target, "cancel")?;
                    for gone in invoked_sessions.remove(target) {
                        if let Some(path) = self.keys.remove(&gone) {
                            self.sessions.remove(&path);
                        }
                    }
                }
            }
            Act::Return(value) => self.session(&path).return_data = Some(value.to_owned()),
            Act::Finish => self.session(&path).finished = true,
        }
        Ok(())
    }
}

impl Sessions for Fake {
    type Id = String;
    type Event = String;
    type Value = String;
    type Due = (Target, &'static str);
    type Input = String;
    type SendId = String;
    type Error = FakeError;

    fn require_budget(&self) -> Result<(), FakeError> {
        if self.log_budget {
            self.log.borrow_mut().push("budget".into());
        }
        if self.steps >= self.max_commands {
            Err(FakeError::Budget)
        } else {
            Ok(())
        }
    }

    fn start_root(&mut self, invoked_sessions: &mut FakeInvocations) -> Result<(), FakeError> {
        self.command(invoked_sessions, SessionKey::ROOT, "start")
    }

    fn send_input(
        &mut self,
        invoked_sessions: &mut FakeInvocations,
        input: &String,
    ) -> Result<(), FakeError> {
        if input.contains(' ') {
            return Err(FakeError::InvalidInput);
        }
        self.command(invoked_sessions, SessionKey::ROOT, input)
    }

    fn deliver(
        &mut self,
        invoked_sessions: &mut FakeInvocations,
        session: SessionKey,
        delivery: Delivery<String, String, String>,
    ) -> Result<(), FakeError> {
        let event = match delivery {
            Delivery::External(event) => event,
            Delivery::FromInvocation {
                invoke_id,
                message: Invoked::Event(event),
            } => format!("{invoke_id}:{event}"),
            Delivery::FromInvocation {
                invoke_id,
                message: Invoked::Done(data),
            } => match data {
                Some(data) => format!("done.invoke.{invoke_id}({data})"),
                None => format!("done.invoke.{invoke_id}"),
            },
            Delivery::FromInvocation {
                invoke_id,
                message: Invoked::CommunicationError,
            } => format!("error.communication.{invoke_id}"),
        };
        self.command(invoked_sessions, session, &event)
    }

    fn communication_error(
        &mut self,
        invoked_sessions: &mut FakeInvocations,
        session: SessionKey,
        send_id: String,
    ) -> Result<(), FakeError> {
        self.command(
            invoked_sessions,
            session,
            &format!("error.communication:{send_id}"),
        )
    }

    fn finished(&self, session: SessionKey) -> bool {
        self.keys
            .get(&session)
            .and_then(|path| self.sessions.get(path))
            .is_some_and(|session| session.finished)
    }

    fn take_return_data(&mut self, session: SessionKey) -> Option<String> {
        self.keyed(session).return_data.take()
    }

    fn next_deadline(&mut self, session: SessionKey) -> Result<Option<u64>, FakeError> {
        self.deadline_reads.set(self.deadline_reads.get() + 1);
        Ok(self
            .keyed(session)
            .pending
            .iter()
            .map(|(deadline, ..)| *deadline)
            .min())
    }

    fn now(&mut self) -> Result<u64, FakeError> {
        Ok(self.session(&[]).now)
    }

    fn advance_clock(&mut self, delta: u64) -> Result<(), FakeError> {
        self.log.borrow_mut().push(format!("tick +{delta}"));
        for session in self.sessions.values_mut() {
            session.now += delta;
        }
        Ok(())
    }

    fn drain_due(&mut self, session: SessionKey) -> Result<Vec<(Target, &'static str)>, FakeError> {
        let fake = self.keyed(session);
        let now = fake.now;
        let mut due: Vec<_> = fake
            .pending
            .iter()
            .filter(|(deadline, ..)| *deadline <= now)
            .copied()
            .collect();
        fake.pending.retain(|(deadline, ..)| *deadline > now);
        due.sort_by_key(|(deadline, sequence, ..)| (*deadline, *sequence));
        Ok(due
            .into_iter()
            .map(|(_, _, target, event)| (target, event))
            .collect())
    }

    fn route_due(
        &mut self,
        invoked_sessions: &mut FakeInvocations,
        session: SessionKey,
        (target, event): (Target, &'static str),
    ) -> Result<DueRoute<String, String, String>, FakeError> {
        Ok(match target {
            Target::Posted => {
                self.log
                    .borrow_mut()
                    .push(format!("{} posted {event}", name(&self.path(session))));
                if !session.is_root() {
                    invoked_sessions.push_to_parent(session, Invoked::Event(event.to_owned()));
                }
                DueRoute::Dispatched
            }
            Target::Local => DueRoute::Local(event.to_owned()),
            Target::Parent if !session.is_root() => DueRoute::Parent(event.to_owned()),
            Target::Parent => DueRoute::Undeliverable(event.to_owned()),
            Target::Child(id) => {
                if invoked_sessions.child(session, &id.to_owned()).is_some() {
                    DueRoute::Invocation(id.to_owned(), event.to_owned())
                } else {
                    DueRoute::Undeliverable(event.to_owned())
                }
            }
        })
    }
}

fn run(fake: &mut Fake, inputs: &[&str]) -> Result<(), FakeError> {
    let inputs: Vec<String> = inputs.iter().map(|input| (*input).to_owned()).collect();
    Scheduler::new().run_scripted(fake, &inputs)
}

fn assert_log(fake: &Fake, expected: &[&str]) {
    assert_eq!(*fake.log.borrow(), expected, "exact session command order");
}

use Act::{At, Cancel, Delay, Finish, Forward, Invoke, Return, ToChild, ToParent};
use Target::{Child, Local, Parent, Posted};

#[test]
fn dispatched_due_sends_route_nothing_and_may_queue_on_other_sessions() {
    // A due send its processor dispatched itself (a BasicHTTP POST) leaves
    // nothing to route; what the dispatch queued elsewhere is delivered in
    // executor order.
    let mut fake = Fake::new(&[
        (
            "root",
            "start",
            &[Invoke("a"), Delay(5, Posted, "root-post")],
        ),
        ("a", "start", &[Delay(5, Posted, "a-post")]),
    ]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &[
            "root<-start",
            "a<-start",
            "tick +5",
            "root posted root-post",
            "a posted a-post",
            "root<-a:a-post",
        ],
    );
}

#[test]
fn scripted_run_starts_delivers_inputs_in_order_then_drains() {
    let mut fake = Fake::new(&[("root", "start", &[Delay(5, Local, "timer")])]);
    run(&mut fake, &["e1", "e2"]).expect("run completes");
    assert_log(
        &fake,
        &[
            "root<-start",
            "root<-e1",
            "root<-e2",
            "tick +5",
            "root<-timer",
        ],
    );
}

#[test]
fn input_after_finish_fails_before_the_input_is_validated() {
    let mut fake = Fake::new(&[("root", "e1", &[Finish])]);
    let error = run(&mut fake, &["e1", "not valid"]).expect_err("input after finish");
    assert_eq!(error, FakeError::Violation(Violation::EventAfterFinish));
    assert_log(&fake, &["root<-start", "root<-e1"]);

    let mut fake = Fake::new(&[]);
    let error = run(&mut fake, &["not valid"]).expect_err("invalid input");
    assert_eq!(error, FakeError::InvalidInput);
    assert_log(&fake, &["root<-start"]);
}

#[test]
fn root_due_sends_drain_before_invoked_due_sends_at_the_same_deadline() {
    let mut fake = Fake::new(&[
        ("root", "start", &[Invoke("a"), Delay(10, Local, "r1")]),
        ("a", "start", &[Delay(10, Local, "a1")]),
    ]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &["root<-start", "a<-start", "tick +10", "root<-r1", "a<-a1"],
    );
}

#[test]
fn only_sessions_due_at_the_earliest_deadline_drain_in_pre_order() {
    let mut fake = Fake::new(&[
        ("root", "start", &[Invoke("b"), Invoke("a")]),
        ("a", "start", &[Delay(4, Local, "a1"), Invoke("x")]),
        ("a/x", "start", &[Delay(4, Local, "x1")]),
        (
            "b",
            "start",
            &[Delay(4, Local, "b1"), Delay(9, Local, "b2")],
        ),
    ]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &[
            "root<-start",
            "b<-start",
            "a<-start",
            "a/x<-start",
            "tick +4",
            "a<-a1",
            "a/x<-x1",
            "b<-b1",
            "tick +5",
            "b<-b2",
        ],
    );
}

#[test]
fn sends_go_straight_onto_the_target_external_queue() {
    let mut fake = Fake::new(&[
        (
            "root",
            "start",
            &[Invoke("a"), ToChild("a", "x1"), ToChild("a", "x2")],
        ),
        ("a", "x1", &[ToParent("p1")]),
    ]);
    let mut checkpoints = Vec::new();
    Scheduler::new()
        .run_scripted_with(
            &mut fake,
            &[],
            |fake: &mut Fake, invoked_sessions: &FakeInvocations, checkpoint| {
                checkpoints.push(format!(
                    "cp{} {:?} {}",
                    checkpoint.ordinal,
                    checkpoint.kind,
                    queues(invoked_sessions)
                ));
                let _ = fake;
                Ok(())
            },
        )
        .expect("run completes");
    assert_log(
        &fake,
        &["root<-start", "a<-start", "a<-x1", "root<-a:p1", "a<-x2"],
    );
    assert_eq!(
        checkpoints,
        [
            "cp0 Start a:[x1,x2]",
            "cp1 Drain a:[x1,x2]",
            "cp2 Drain root:[a:p1] a:[x2]",
            "cp3 Drain a:[x2]",
            "cp4 Drain ",
            "cp5 End ",
        ]
    );
}

/// Every non-empty queue as `session:[entries]`, in invocation order.
fn queues(invoked_sessions: &FakeInvocations) -> String {
    invoked_sessions
        .keys()
        .into_iter()
        .filter_map(|key| {
            let entries: Vec<String> = invoked_sessions
                .external(key)
                .into_iter()
                .flatten()
                .map(|delivery| match delivery {
                    Delivery::External(event) => event.clone(),
                    Delivery::FromInvocation {
                        invoke_id,
                        message: Invoked::Event(event),
                    } => format!("{invoke_id}:{event}"),
                    Delivery::FromInvocation {
                        invoke_id,
                        message: Invoked::Done(_),
                    } => format!("done.invoke.{invoke_id}"),
                    Delivery::FromInvocation {
                        invoke_id,
                        message: Invoked::CommunicationError,
                    } => format!("error.communication.{invoke_id}"),
                })
                .collect();
            let path = invoked_sessions.path(key).unwrap_or_default();
            (!entries.is_empty()).then(|| format!("{}:[{}]", name(&path), entries.join(",")))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn done_invoke_follows_every_event_the_session_sent_case_236() {
    let mut fake = Fake::new(&[
        ("root", "start", &[Invoke("a")]),
        (
            "a",
            "start",
            &[ToParent("e1"), ToParent("e2"), Return("v"), Finish],
        ),
    ]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &[
            "root<-start",
            "a<-start",
            "root<-a:e1",
            "root<-a:e2",
            "root<-done.invoke.a(v)",
        ],
    );
}

#[test]
fn done_invoke_waits_for_parent_sends_from_the_finishing_due_batch() {
    let mut fake = Fake::new(&[
        ("root", "start", &[Invoke("a")]),
        (
            "a",
            "start",
            &[Delay(3, Parent, "late"), Delay(3, Local, "finish")],
        ),
        ("a", "finish", &[Finish]),
    ]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &[
            "root<-start",
            "a<-start",
            "tick +3",
            "a<-finish",
            "root<-a:late",
            "root<-done.invoke.a",
        ],
    );
}

#[test]
fn a_session_being_cancelled_sends_nothing_to_its_parent_case_252() {
    let mut fake = Fake::new(&[
        (
            "root",
            "start",
            &[Invoke("a"), Cancel("a"), Delay(1, Local, "timeout")],
        ),
        ("a", "start", &[ToParent("early")]),
        ("a", "cancel", &[ToParent("on-exit")]),
    ]);
    run(&mut fake, &[]).expect("run completes");
    // `early` was queued before the cancel and stays; the cancel's own send
    // is ignored; no done.invoke follows a cancelled session.
    assert_log(
        &fake,
        &[
            "root<-start",
            "a<-start",
            "a<-cancel",
            "root<-a:early",
            "tick +1",
            "root<-timeout",
        ],
    );
}

#[test]
fn a_due_send_to_a_cancelled_session_is_undeliverable_to_its_sender() {
    // SCXML 6.2: the message is dispatched only when the delay elapses;
    // 6.2.4: unable to dispatch, the sender gets error.communication.
    let mut fake = Fake::new(&[
        (
            "root",
            "start",
            &[
                Invoke("a"),
                Delay(5, Child("a"), "late"),
                Delay(1, Local, "cancel"),
            ],
        ),
        ("root", "cancel", &[Cancel("a")]),
    ]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &[
            "root<-start",
            "a<-start",
            "tick +1",
            "root<-cancel",
            "a<-cancel",
            "tick +4",
            "root<-error.communication:late",
        ],
    );
}

#[test]
fn autoforward_and_sends_reach_the_invoked_queue_in_send_order() {
    let mut fake = Fake::new(&[(
        "root",
        "start",
        &[Invoke("a"), Forward("a", "f1"), ToChild("a", "s1")],
    )]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(&fake, &["root<-start", "a<-start", "a<-f1", "a<-s1"]);
}

#[test]
fn nested_sessions_message_across_levels_in_pre_order() {
    let mut fake = Fake::new(&[
        ("root", "start", &[Invoke("a")]),
        ("a", "start", &[Invoke("x")]),
        ("a/x", "start", &[ToParent("up")]),
        ("a", "x:up", &[ToParent("relay")]),
        ("root", "a:relay", &[ToChild("a", "down")]),
        ("a", "down", &[ToChild("x", "deep")]),
        ("a/x", "deep", &[Finish]),
        ("a", "done.invoke.x", &[Finish]),
    ]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &[
            "root<-start",
            "a<-start",
            "a/x<-start",
            "a<-x:up",
            "root<-a:relay",
            "a<-down",
            "a/x<-deep",
            "a<-done.invoke.x",
            "root<-done.invoke.a",
        ],
    );
}

#[test]
fn cancelling_a_session_removes_every_session_it_invoked() {
    let mut fake = Fake::new(&[
        ("root", "start", &[Invoke("a"), Delay(1, Local, "stop")]),
        ("a", "start", &[Invoke("x"), Delay(5, Local, "a-late")]),
        ("a/x", "start", &[Delay(5, Local, "x-late")]),
        ("a", "cancel", &[Cancel("x")]),
        ("a/x", "cancel", &[ToParent("ignored")]),
        ("root", "stop", &[Cancel("a")]),
    ]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &[
            "root<-start",
            "a<-start",
            "a/x<-start",
            "tick +1",
            "root<-stop",
            "a<-cancel",
            "a/x<-cancel",
        ],
    );
    assert_eq!(fake.sessions.len(), 1, "only the root remains");
}

#[test]
fn embedder_invoke_limits_bound_depth_and_invoked_sessions() {
    let mut invoked_sessions = FakeInvocations::new(InvokeLimits {
        max_invoke_depth: Some(2),
        max_invoked_sessions: Some(2),
    });
    let a = invoked_sessions
        .register(SessionKey::ROOT, "a".into())
        .expect("depth 1");
    let x = invoked_sessions.register(a, "x".into()).expect("depth 2");
    assert!(
        !invoked_sessions.can_invoke(x),
        "depth 3 is beyond the bound"
    );
    assert_eq!(
        invoked_sessions.register(x, "y".into()),
        Err(Violation::InvokeLimit)
    );
    assert_eq!(
        invoked_sessions.register(SessionKey::ROOT, "b".into()),
        Err(Violation::InvokeLimit)
    );
    assert_eq!(
        invoked_sessions.register(a, "x".into()),
        Err(Violation::DuplicateInvocation)
    );
    assert_eq!(invoked_sessions.remove(a), [x, a], "deepest first");
    assert_eq!(
        invoked_sessions.register(a, "c".into()),
        Err(Violation::SessionMissing)
    );
    assert_eq!(invoked_sessions.len(), 1);
    invoked_sessions
        .register(SessionKey::ROOT, "b".into())
        .expect("room again");
    let mut closed = FakeInvocations::new(InvokeLimits {
        max_invoke_depth: Some(0),
        max_invoked_sessions: None,
    });
    assert_eq!(
        closed.register(SessionKey::ROOT, "a".into()),
        Err(Violation::InvokeLimit),
        "a zero depth lets no session invoke"
    );
}

/// The scheduler imposes no invoke limit of its own: by default a session
/// invokes deeper than 32 levels and more than 255 sessions live at once.
#[test]
fn default_invoke_limits_are_unbounded() {
    assert_eq!(
        InvokeLimits::default(),
        InvokeLimits {
            max_invoke_depth: None,
            max_invoked_sessions: None,
        }
    );
    let mut invoked_sessions = FakeInvocations::default();
    let mut deepest = SessionKey::ROOT;
    for depth in 0..40 {
        deepest = invoked_sessions
            .register(deepest, format!("d{depth}"))
            .expect("no host-imposed depth limit");
    }
    assert_eq!(invoked_sessions.depth(deepest), Some(40));
    for sibling in 0..300 {
        invoked_sessions
            .register(SessionKey::ROOT, format!("s{sibling}"))
            .expect("no host-imposed invoked-session limit");
    }
    assert_eq!(invoked_sessions.invoked_len(), 340);
    assert!(invoked_sessions.can_invoke(deepest));
}

/// External queues hold every event sent: more than 10,000 entries queue.
#[test]
fn external_queues_are_unbounded() {
    let mut invoked_sessions = FakeInvocations::default();
    let a = invoked_sessions
        .register(SessionKey::ROOT, "a".into())
        .expect("a");
    for index in 0..=10_000 {
        assert!(invoked_sessions.push_external(a, format!("e{index}")));
    }
    assert!(invoked_sessions.push_to_parent(a, Invoked::Event("up".into())));
    assert_eq!(invoked_sessions.external_len(a), Some(10_001));
    assert_eq!(invoked_sessions.external_len(SessionKey::ROOT), Some(1));
    invoked_sessions.remove(a);
    assert!(!invoked_sessions.push_external(a, "x".into()));
}

#[test]
fn finished_sessions_receive_no_input_and_their_deadlines_are_ignored() {
    let mut fake = Fake::new(&[
        (
            "root",
            "start",
            &[Invoke("a"), ToChild("a", "stop"), ToChild("a", "after")],
        ),
        ("a", "start", &[Delay(9, Local, "never")]),
        ("a", "stop", &[Finish]),
    ]);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &["root<-start", "a<-start", "a<-stop", "root<-done.invoke.a"],
    );
}

#[test]
fn a_deadline_before_the_shared_clock_is_a_harness_violation() {
    let mut fake = Fake::new(&[
        ("root", "start", &[Delay(10, Local, "tick")]),
        ("root", "tick", &[At(3, "late")]),
    ]);
    let error = run(&mut fake, &[]).expect_err("clock invariant");
    assert_eq!(error, FakeError::Violation(Violation::SendClockInvariant));
    assert_log(&fake, &["root<-start", "tick +10", "root<-tick"]);
}

#[test]
fn budget_is_required_at_each_drain_pass_and_before_each_due_send() {
    let mut fake = Fake::new(&[(
        "root",
        "start",
        &[Delay(5, Local, "x"), Delay(5, Local, "y")],
    )])
    .with_budget(10);
    run(&mut fake, &[]).expect("run completes");
    assert_log(
        &fake,
        &[
            "root<-start",
            "budget",
            "tick +5",
            "budget",
            "root<-x",
            "budget",
            "root<-y",
            "budget",
        ],
    );
    let mut fake = Fake::new(&[("root", "start", &[Delay(5, Local, "x")])]).with_budget(1);
    assert_eq!(run(&mut fake, &[]), Err(FakeError::Budget));
}

#[test]
fn external_client_arrivals_deliver_in_order_until_the_root_finishes() {
    let mut fake = Fake::new(&[("root", "a2", &[Finish])]);
    let response = Scheduler::new()
        .run_external_client(&mut fake, |_, arrivals| {
            for event in ["a1", "a2", "a3"] {
                arrivals.push_back(event.to_owned());
            }
            Ok("ok")
        })
        .expect("run completes");
    assert_eq!(response, "ok");
    assert_log(&fake, &["root<-start", "root<-a1", "root<-a2"]);
}

/// The external client's arrival queue holds every post: more than 10,000
/// arrivals are all delivered.
#[test]
fn external_client_arrivals_are_unbounded() {
    let mut fake = Fake::new(&[]);
    fake.max_commands = 20_000;
    Scheduler::new()
        .run_external_client(&mut fake, |_, arrivals| {
            for index in 0..=10_000 {
                arrivals.push_back(format!("a{index}"));
            }
            Ok(())
        })
        .expect("run completes");
    let log = fake.log.borrow();
    assert_eq!(log.len(), 10_002, "start, then every arrival");
    assert_eq!(log.last().map(String::as_str), Some("root<-a10000"));
}

#[test]
fn a_bounded_horizon_waits_at_the_first_later_deadline() {
    let mut fake = Fake::new(&[(
        "root",
        "start",
        &[Delay(5, Local, "early"), Delay(20, Local, "late")],
    )]);
    let mut scheduler = Scheduler::new();
    let mut hook = no_checkpoint::<Fake>();
    scheduler.start(&mut fake, &mut hook).expect("start");
    assert_eq!(
        scheduler.drain_until(&mut fake, Horizon::Until(4), &mut hook),
        Ok(DrainStop::Waiting { deadline: 5 })
    );
    assert_eq!(
        scheduler.drain_until(&mut fake, Horizon::Until(10), &mut hook),
        Ok(DrainStop::Waiting { deadline: 20 })
    );
    scheduler
        .advance_clock_to(&mut fake, 10)
        .expect("nothing is due before 10");
    assert_eq!(scheduler.next_deadline(&mut fake), Ok(Some(20)));
    assert_eq!(
        scheduler.drain_until(&mut fake, Horizon::Unbounded, &mut hook),
        Ok(DrainStop::Idle)
    );
    assert_log(
        &fake,
        &[
            "root<-start",
            "tick +5",
            "root<-early",
            "tick +5",
            "tick +10",
            "root<-late",
        ],
    );
}

#[test]
fn advancing_the_clock_fails_closed_backwards_or_past_a_pending_deadline() {
    let mut fake = Fake::new(&[("root", "start", &[Delay(5, Local, "timer")])]);
    let mut scheduler = Scheduler::new();
    let mut hook = no_checkpoint::<Fake>();
    scheduler.start(&mut fake, &mut hook).expect("start");
    assert_eq!(
        scheduler.advance_clock_to(&mut fake, 6),
        Err(FakeError::Violation(Violation::SendClockInvariant))
    );
    scheduler
        .advance_clock_to(&mut fake, 5)
        .expect("a deadline at the target stays pending");
    assert_eq!(
        scheduler.advance_clock_to(&mut fake, 4),
        Err(FakeError::Violation(Violation::ClockRegression))
    );
}

#[test]
fn identical_sessions_schedule_identically_and_a_hook_changes_no_call() {
    let scenario = || {
        Fake::new(&[
            (
                "root",
                "start",
                &[Invoke("b"), Invoke("a"), Delay(4, Child("a"), "p")],
            ),
            (
                "a",
                "start",
                &[Delay(4, Parent, "q"), ToParent("r"), Invoke("x")],
            ),
            ("a/x", "start", &[ToParent("deep")]),
            ("b", "start", &[Delay(2, Local, "s"), Return("v"), Finish]),
            ("a", "p", &[Finish]),
        ])
    };
    let mut first = scenario();
    let mut second = scenario();
    let mut hooked = scenario();
    run(&mut first, &[]).expect("first");
    run(&mut second, &[]).expect("second");
    Scheduler::new()
        .run_scripted_with(&mut hooked, &[], |_: &mut Fake, _: &FakeInvocations, _| {
            Ok(())
        })
        .expect("hooked");
    assert_eq!(*first.log.borrow(), *second.log.borrow());
    assert_eq!(*first.log.borrow(), *hooked.log.borrow());
}

#[test]
fn checkpoint_ordinals_fail_closed_past_u32() {
    let mut scheduler = Scheduler::<Fake>::new();
    scheduler.next_checkpoint = Some(u32::MAX);
    let mut fake = Fake::new(&[]);
    let mut hook = no_checkpoint::<Fake>();
    scheduler
        .reach(&mut fake, &mut hook, CheckpointKind::Drain, None)
        .expect("the last ordinal is usable");
    assert_eq!(
        scheduler.reach(&mut fake, &mut hook, CheckpointKind::Drain, None),
        Err(FakeError::Violation(Violation::CheckpointLimit))
    );
}

#[test]
fn violation_codes_and_messages_are_stable() {
    for (violation, code) in [
        (Violation::EventAfterFinish, "event-after-finish"),
        (Violation::SendClockInvariant, "send-clock-invariant"),
        (Violation::InvokeLimit, "invoke-limit-exceeded"),
        (Violation::SessionMissing, "session-missing"),
        (Violation::CheckpointLimit, "checkpoint-limit"),
        (Violation::ClockRegression, "clock-regression"),
    ] {
        assert_eq!(violation.code(), code);
        assert_eq!(violation.to_string(), violation.message());
    }
}

/// The invoke tree is walked iteratively: a chain 100,000 sessions deep is
/// registered, ordered, cancelled, removed deepest first, and dropped
/// without native stack per level.
#[test]
fn a_chain_of_any_depth_is_walked_without_recursion() {
    const DEPTH: usize = 100_000;
    let mut invoked_sessions = FakeInvocations::default();
    let mut chain = vec![SessionKey::ROOT];
    for depth in 0..DEPTH {
        let parent = *chain.last().expect("the chain is never empty");
        chain.push(
            invoked_sessions
                .register(parent, format!("k{depth}"))
                .expect("no limit"),
        );
    }
    let deepest = *chain.last().expect("the chain is never empty");
    assert_eq!(invoked_sessions.depth(deepest), Some(DEPTH));
    assert_eq!(invoked_sessions.keys(), chain, "pre-order is the chain");
    assert!(invoked_sessions.precedes(chain[1], deepest));
    assert!(!invoked_sessions.precedes(deepest, chain[1]));
    assert_eq!(
        invoked_sessions.path(deepest).map(|path| path.len()),
        Some(DEPTH)
    );
    invoked_sessions.begin_cancel(chain[1]);
    assert!(!invoked_sessions.push_to_parent(deepest, Invoked::Event("late".into())));
    let removed = invoked_sessions.remove(chain[1]);
    let mut expected = chain[1..].to_vec();
    expected.reverse();
    assert_eq!(removed, expected, "deepest first");
    assert_eq!(invoked_sessions.len(), 1);
    // Dropping a deep tree is flat as well.
    let mut deep = FakeInvocations::default();
    let mut parent = SessionKey::ROOT;
    for depth in 0..DEPTH {
        parent = deep
            .register(parent, format!("k{depth}"))
            .expect("no limit");
    }
    drop(deep);
}

/// One drain iteration reads the status and deadline only of the sessions
/// a command ran on since the last one: idle invoked sessions cost nothing
/// per iteration, however many there are.
#[test]
fn a_drain_iteration_reads_only_the_sessions_a_command_ran_on() {
    let children: Vec<&'static str> = (0..50)
        .map(|index| &*Box::leak(format!("c{index}").into_boxed_str()))
        .collect();
    let mut start: Vec<Act> = children.iter().map(|id| Invoke(id)).collect();
    start.push(Delay(1, Local, "t1"));
    let start: &'static [Act] = Box::leak(start.into_boxed_slice());
    let mut reactions: Vec<(&'static str, &str, &[Act])> = vec![("root", "start", start)];
    let ticks: Vec<(&'static str, &'static str)> = (1..20)
        .map(|index| {
            (
                &*Box::leak(format!("t{index}").into_boxed_str()),
                &*Box::leak(format!("t{}", index + 1).into_boxed_str()),
            )
        })
        .collect();
    for (tick, next) in &ticks {
        reactions.push((
            "root",
            tick,
            Box::leak(vec![Delay(1, Local, next)].into_boxed_slice()),
        ));
    }
    let mut fake = Fake::new(&reactions);
    run(&mut fake, &[]).expect("run completes");
    assert!(fake.log.borrow().iter().any(|entry| entry == "root<-t20"));
    let reads = fake.deadline_reads.get();
    assert!(
        reads < 100,
        "20 root iterations over 50 idle sessions read {reads} deadlines"
    );
}

/// Each session's place in invocation order, fixed when it registers, is
/// the order of the invoke-id paths whatever order sessions register and
/// leave in (siblings out of id order, chains, removals): comparisons,
/// the session list, and the order queued entries are taken in all agree
/// with sorting the paths.
#[test]
fn invocation_order_is_the_path_order_under_any_registration_order() {
    let mut invoked_sessions = FakeInvocations::default();
    let mut live = vec![SessionKey::ROOT];
    // A fixed linear congruential sequence: deterministic, no dependency.
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = |bound: usize| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(seed >> 33).expect("31 bits fit") % bound
    };
    let by_path = |invoked_sessions: &FakeInvocations, keys: &[SessionKey]| {
        let mut sorted = keys.to_vec();
        sorted.sort_by_key(|key| invoked_sessions.path(*key).expect("present"));
        sorted
    };
    // A spine 200 levels deep whose every level registers `b` before `a`:
    // the gap between neighbouring labels halves at every level, so the
    // labels must be spread again, and a registration order that differs
    // from the invoke-id order must not leak into the result.
    let mut spine = SessionKey::ROOT;
    for _ in 0..200 {
        let later = invoked_sessions
            .register(spine, "b".into())
            .expect("no limit");
        let earlier = invoked_sessions
            .register(spine, "a".into())
            .expect("no limit");
        live.extend([later, earlier]);
        spine = later;
    }
    for round in 0..4_000 {
        if round % 7 == 6 && live.len() > 1 {
            let gone = live[1 + next(live.len() - 1)];
            let removed = invoked_sessions.remove(gone);
            live.retain(|key| !removed.contains(key));
        } else {
            let parent = live[next(live.len())];
            // Ids from a small alphabet, so siblings arrive out of order
            // and labels run out of room between neighbours.
            let id = format!("{}", next(40));
            if let Ok(key) = invoked_sessions.register(parent, id) {
                live.push(key);
            }
        }
        if round % 250 == 0 || round == 3_999 {
            let expected = by_path(&invoked_sessions, &live);
            assert_eq!(invoked_sessions.keys(), expected, "round {round}");
            for pair in expected.windows(2) {
                assert!(invoked_sessions.precedes(pair[0], pair[1]), "round {round}");
                assert!(
                    !invoked_sessions.precedes(pair[1], pair[0]),
                    "round {round}"
                );
            }
        }
    }
    // Queue one entry on every session in reverse order: they are taken in
    // invocation order.
    let expected = by_path(&invoked_sessions, &live);
    for key in expected.iter().rev() {
        invoked_sessions.push_external(*key, "e".into());
    }
    let mut taken = Vec::new();
    while let Some((key, _)) = invoked_sessions.pop_next(|_| true) {
        taken.push(key);
    }
    assert_eq!(taken, expected);
}

/// The scheduler work steps (`crate::work`) to build a chain `depth`
/// sessions deep in which every level sends to its invoking session at
/// once, take every entry, and cancel and remove the chain below the root.
/// Every entry is taken in invocation order, root first.
fn deep_chain_work(depth: usize) -> u64 {
    let mut invoked_sessions = FakeInvocations::default();
    let mut chain = vec![SessionKey::ROOT];
    crate::work::take();
    for level in 0..depth {
        let parent = *chain.last().expect("the chain is never empty");
        let key = invoked_sessions
            .register(parent, format!("k{level}"))
            .expect("no limit");
        assert!(invoked_sessions.push_to_parent(key, Invoked::Event("up".into())));
        chain.push(key);
    }
    let mut taken = Vec::new();
    while let Some((key, _)) = invoked_sessions.pop_next(|_| true) {
        taken.push(key);
    }
    invoked_sessions.begin_cancel(chain[1]);
    assert_eq!(invoked_sessions.remove(chain[1]).len(), depth);
    let work = crate::work::take();
    assert_eq!(taken, chain[..depth], "each invoking session, root first");
    work
}

/// Ordering queued work costs no more than the queue and the tree: in
/// chains 1,250 to 10,000 sessions deep, every level sends to its invoking
/// session at once and every entry is taken in invocation order, and the
/// scheduler's counted work steps grow as `n log n` (order-maintenance
/// relabeling is amortized logarithmic per session): at most
/// `12 n ceil(log2 n)` steps (measured: 112,003, 277,691, 585,233, and
/// 1,238,945 steps, about 9 n log2 n), and each doubling of the depth at
/// most 3 times the steps (measured 2.1 to 2.5; a scan of the chain per
/// level is 4 times).
#[test]
fn a_deep_chain_with_every_level_queued_is_taken_in_order_in_n_log_n_steps() {
    let depths: [u64; 4] = [1_250, 2_500, 5_000, 10_000];
    let work: Vec<u64> = depths
        .iter()
        .map(|depth| deep_chain_work(usize::try_from(*depth).expect("small")))
        .collect();
    for (pair, depth) in work.windows(2).zip(&depths[1..]) {
        assert!(
            pair[1] <= pair[0] * 3,
            "{depth} levels took {} steps, {} at half the depth ({work:?})",
            pair[1],
            pair[0]
        );
    }
    for (steps, depth) in work.iter().zip(depths) {
        let log = u64::from(depth.next_power_of_two().trailing_zeros());
        assert!(
            *steps <= 12 * depth * log,
            "{depth} levels took {steps} steps, over 12 n log2 n ({work:?})"
        );
    }
}

/// An invoke id that counts how often it is compared for equality.
#[derive(Clone, Debug)]
struct CountedId(&'static str, std::rc::Rc<std::cell::Cell<usize>>);

impl PartialEq for CountedId {
    fn eq(&self, other: &Self) -> bool {
        self.1.set(self.1.get() + 1);
        self.0 == other.0
    }
}

impl Eq for CountedId {}

impl PartialOrd for CountedId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CountedId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(other.0)
    }
}

/// `has_path` compares a session's invoke ids from the root with a path
/// without building the session's own: a path of another length is no
/// match at once, and a mismatch is found walking up from the session, so
/// a session that differs at its own invoke id costs one comparison.
#[test]
fn a_session_path_is_compared_from_the_session_up_and_stops_at_the_first_mismatch() {
    let compared = std::rc::Rc::new(std::cell::Cell::new(0));
    let id = |name| CountedId(name, std::rc::Rc::clone(&compared));
    let mut invoked: InvokedSessions<CountedId, String, String> = InvokedSessions::default();
    let mut key = SessionKey::ROOT;
    for name in ["a", "b", "c", "d"] {
        key = invoked.register(key, id(name)).expect("unlimited");
    }
    compared.set(0);
    assert!(invoked.has_path(key, &[id("a"), id("b"), id("c"), id("d")]));
    assert_eq!(compared.get(), 4);
    compared.set(0);
    assert!(!invoked.has_path(key, &[id("a"), id("b"), id("c")]));
    assert_eq!(compared.get(), 0, "another length is no match at once");
    compared.set(0);
    assert!(!invoked.has_path(key, &[id("a"), id("b"), id("c"), id("x")]));
    assert_eq!(compared.get(), 1, "the session's own invoke id differs");
    assert!(invoked.has_path(SessionKey::ROOT, &[]));
    assert!(!invoked.has_path(SessionKey::ROOT, &[id("a")]));
}
