# The routines_runner's statements and cases (tests/irp/harness.py writes them).

S = suite
TICK = "bot.behavior.tick"
AT = "2026-10-05T08:00:00-04:00"


def tick_expr(name, text):
    return (f"({{name: '{name}', trigger: 'every', scheduled_at: '{AT}', fired_at: '{AT}', "
            f"output: [{{event: 'bot.speaking.say', target: 'speaker', data: {{text: '{text}'}}}}]}})")


def tick_send(name, text):
    return f'<send event="bot.behavior.tick" target="#_parent"><content expr="{tick_expr(name, text)}"/></send>'


def routine(body, initial="s"):
    return ('<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" '
            f'initial="{initial}">{body}</scxml>')


def once(name, text="once", as_name=None):
    """Ticks once, as `as_name` (default its own name), and ends."""
    return routine(f'<state id="s"><onentry>{tick_send(as_name or name, text)}</onentry>'
                   '<transition target="end"/></state><final id="end"/>')


def every(name, text="every"):
    """Ticks every 60 s, never ending."""
    return routine('<state id="s"><onentry><send event="every" delay="60s"/></onentry>'
                   f'<transition event="every" target="s">{tick_send(name, text)}</transition></state>')


def chain(*rows):
    return [("routines", js([{"name": name, "source": S.behavior(source)} for name, source in rows]))]


def ticked(name, text):
    return expect(TICK, f"_event.data.name === {js(name)} && _event.data.output[0].data.text === {js(text)}")


# On the logical clock a delayed send to itself fires once the run is idle:
# a step after which nothing else arrives.
QUIET = wait("1s")


S.statement(1, "It takes the `<param>` `routines` `[{name, source}]` (`source` the path of the routine's program, decision D121) and runs the first routine as its child `routine` (`<invoke srcexpr>` on its source); on start it sends nothing of its own, and it never reaches a final state.")
S.statement(2, "Each `bot.behavior.tick` its routine sends it goes to its parent unchanged, once.")
S.statement(3, "With more than one routine it invokes itself (`rest`) with the rest of the routines, so every routine runs and each tick of the chain reaches the parent once, whatever its depth.")
S.statement(4, "A tick whose `name` is none of the chain's routines' names is not forwarded (the original's stale tick).")
S.statement(5, "A routine that ends (`done.invoke.routine`) stays stopped, and the rest keep running.")
S.statement(6, "A routine whose program does not compile (`error.execution` in the runner) stays stopped, logged `routines.failed` `<name> (error.execution)`, and the rest keep running.")
S.statement(7, "A tick the routine sends whose data its schema rejects never reaches the runner (`error.execution` in the routine) and is not forwarded; the routine's later ticks are.")

S.case(100, 1, "a routine that never ticks: the runner sends nothing.",
       QUIET, params=chain(("Quiet", routine('<state id="s"/>'))))
S.case(101, 1, "an empty routines list: the first routine cannot start and nothing is sent.",
       QUIET, params=[("routines", "[]")])
S.case(110, 2, "the routine's tick reaches the parent with all its fields.",
       [expect(TICK, f"_event.data.name === 'Alpha' && _event.data.trigger === 'every' && _event.data.scheduled_at === {js(AT)}"
                     f" && _event.data.fired_at === {js(AT)} && _event.data.output.length === 1"
                     " && _event.data.output[0].event === 'bot.speaking.say' && _event.data.output[0].target === 'speaker'"
                     " && _event.data.output[0].data.text === 'once'")] + QUIET,
       params=chain(("Alpha", once("Alpha"))))
S.case(111, 2, "a routine on a schedule keeps ticking, each tick forwarded once.",
       [ticked("Alpha", "every"), ticked("Alpha", "every"), ticked("Alpha", "every")],
       params=chain(("Alpha", every("Alpha"))))
S.case(120, 3, "three routines each tick once through two nested runners.",
       [expect(TICK, assigns=[("seen", "seen.concat([_event.data.name])")]),
        expect(TICK, assigns=[("seen", "seen.concat([_event.data.name])")]),
        expect(TICK, assigns=[("seen", "seen.concat([_event.data.name])")])] + QUIET +
       [send("harness.check", None, target="self"), expect("harness.check", "seen.sort().join(' ') === 'Alpha Beta Gamma'")],
       data=[("seen", "[]")],
       params=chain(("Alpha", once("Alpha")), ("Beta", once("Beta")), ("Gamma", once("Gamma"))))
S.case(130, 4, "a tick named for no routine of the chain is dropped; the routine's own is forwarded.",
       [ticked("Alpha", "own")] + QUIET,
       params=chain(("Alpha", routine(f'<state id="s"><onentry>{tick_send("Zeta", "stale")}{tick_send("Alpha", "own")}</onentry></state>'))))
S.case(131, 4, "a nested runner drops a tick named for a routine above it: the chain [Alpha, Beta] does not forward Beta's routine ticking as Alpha.",
       [ticked("Alpha", "first")] + QUIET,
       params=chain(("Alpha", once("Alpha", "first")), ("Beta", once("Beta", "posing", as_name="Alpha"))))
S.case(140, 5, "a routine that ends stays stopped while the next keeps ticking.",
       [ticked("Alpha", "once"), ticked("Beta", "every"), ticked("Beta", "every")],
       params=chain(("Alpha", once("Alpha")), ("Beta", every("Beta"))))
S.case(150, 6, "a source that is not SCXML stays stopped and the rest run.",
       [ticked("Beta", "once")] + QUIET,
       params=chain(("Epsilon", "not scxml"), ("Beta", once("Beta"))))
S.case(151, 6, "a broken routine deeper in the chain stops only itself.",
       [expect(TICK, assigns=[("seen", "seen.concat([_event.data.name])")]),
        expect(TICK, assigns=[("seen", "seen.concat([_event.data.name])")])] + QUIET +
       [send("harness.check", None, target="self"), expect("harness.check", "seen.sort().join(' ') === 'Alpha Gamma'")],
       data=[("seen", "[]")],
       params=chain(("Alpha", once("Alpha")), ("Epsilon", "<scxml"), ("Gamma", once("Gamma"))))
S.case(160, 7, "a tick with an empty output is refused in the routine; the routine goes on and its next tick is forwarded.",
       [ticked("Alpha", "valid")] + QUIET,
       params=chain(("Alpha", routine(
           '<state id="s"><onentry><send event="bot.behavior.tick" target="#_parent"><content expr="'
           f"({{name: 'Alpha', trigger: 'every', scheduled_at: '{AT}', fired_at: '{AT}', output: []}})"
           '"/></send></onentry><transition target="t"/></state>'
           f'<state id="t"><onentry>{tick_send("Alpha", "valid")}</onentry></state>'))))
