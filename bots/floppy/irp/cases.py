# The floppy body's statements and cases (tests/irp/harness.py writes them).
#
# The body sends nothing to a parent (each event it emits is a <log>), so a
# case observes it through its one outcome a parent receives: the final
# state degraded, whose donedata {reason} arrives as done.invoke.floppy.
# Every event name the body takes has one meaning whoever sends it, so a
# case sends the abilities' replies and the body's own timers itself, in one
# burst the body takes before its real abilities answer:
#
#   probe(k): a bot.input, then bot.processing.timed_out and
#     bot.processing.cancel.timed_out for turn k: when the bot is active and
#     the input is turn k, the cancel goes unacknowledged and the body ends
#     in degraded with "turn k: the cognition did not acknowledge its cancel";
#     otherwise the probe is not taken and the case waits (and fails).
#   TAKE_TURN: a bot.input answered at once by bot.ability.failed for turn
#     1: when the bot is active it uses up turn 1, so a later probe(1)
#     cannot pass; when it is not, nothing happens.

S = suite
ACTIVATE = "bot.activate"
DEACTIVATE = "bot.deactivate"
OFFERS = "bot.ability.offers"
INPUT = "bot.input"
DONE = "done.invoke.floppy"
ABILITIES = ["cognition", "communication", "identity", "listening"]
GHOST = ("expected", js(ABILITIES + ["ghost"]))


def offers(*abilities):
    return [send(OFFERS, js({"ability": a, "choices": []})) for a in (abilities or ABILITIES)]


def bot_input(text="hello"):
    return send(INPUT, js({"observation": {"event": "text", "data": text}}))


def turn_event(event, k, **extra):
    return send(event, js(dict({"turn": k}, **extra)))


def degraded(k):
    return expect(DONE, f"_event.data.reason === {js(f'turn {k}: the cognition did not acknowledge its cancel')}")


def probe(k):
    return [bot_input("probe"), turn_event("bot.processing.timed_out", k), turn_event("bot.processing.cancel.timed_out", k), degraded(k)]


TAKE_TURN = [bot_input("taken"), turn_event("bot.ability.failed", 1, message="taken")]
ACTIVE = [send(ACTIVATE, js({}))] + offers()

S.statement(1, "`bot.activate` enters `live`, which invokes the abilities (`cognition`, `communication`, `identity`, `listening`); the bot is `active`, and takes a turn's input, only once every expected ability (the data `expected`) has sent its first `bot.ability.offers`.")
S.statement(2, "While `activating`, an `error.*` event, an ability's `done.invoke`, or the activation timer (`activation_timeout_ms`) fails the activation back to `inactive`, where no input is taken until the next `bot.activate`.")
S.statement(3, "`bot.deactivate` leaves `live` for `inactive`, from `active` or `activating`; a later `bot.activate` makes the bot live again.")
S.statement(4, "`bot.reboot` `{reason}` re-enters `live`: the abilities are invoked again and the bot waits for their offers again before it takes input; the turn count goes on.")
S.statement(5, "In `unfocused` or `focused`, `bot.input` or a `bot.ability.cognition.input` `{stimulus}` handoff starts a `processing` turn, counted from 1; the cognition's `bot.ability.cognition.output` or `bot.ability.failed` for the current turn ends it (back to attention), and one for another turn is dropped.")
S.statement(6, "`bot.processing.timed_out` for the current turn moves to `cancelling_processing`; the acknowledgement (`bot.ability.cognition.cancelled` or `bot.ability.processing.cancelled` for the turn) ends the turn; `bot.processing.cancel.timed_out` for it degrades the bot: the session ends in the final state `degraded` with `{reason: \"turn <n>: the cognition did not acknowledge its cancel\"}`. A timer for another turn is not taken.")
S.statement(7, "An input that arrives during a turn waits and starts the next turn when the bot returns to attention.")
S.statement(8, "An ability that ends while the bot is active (`done.invoke.<id>`) degrades it: the session ends in `degraded` with `{reason: \"the ability <id> ended while active\"}`.")

S.case(100, 1, "activated by every ability's offers, the bot takes a turn: probe(1) degrades it at turn 1.",
       ACTIVE + probe(1))
S.case(101, 1, "with an expected ability that has not offered, the bot takes no input; once it offers, it does.",
       [send(ACTIVATE, js({}))] + offers() + TAKE_TURN + offers("ghost") + probe(1),
       params=[GHOST])

S.case(110, 2, "an error while activating fails the activation: no input is taken until bot.activate.",
       [send(ACTIVATE, js({})), send("error.platform", js({}))] + offers() + TAKE_TURN + ACTIVE + probe(1))
S.case(111, 2, "an ability ending while activating fails the activation.",
       [send(ACTIVATE, js({})), send("done.invoke.listening", js({}))] + offers() + TAKE_TURN + ACTIVE + probe(1))
S.case(112, 2, "an activation that times out (an expected ability never offers) fails back to inactive.",
       [send(ACTIVATE, js({})), wait("1s")] + offers(*ABILITIES, "ghost") + TAKE_TURN
       + [send(ACTIVATE, js({}))] + offers(*ABILITIES, "ghost") + probe(1),
       params=[GHOST, ("activation_timeout_ms", "100")])

S.case(120, 3, "bot.deactivate from active leaves live: no input is taken until bot.activate.",
       ACTIVE + [send(DEACTIVATE, js({}))] + offers() + TAKE_TURN + ACTIVE + probe(1))
S.case(121, 3, "bot.deactivate while activating returns to inactive.",
       [send(ACTIVATE, js({})), send(DEACTIVATE, js({}))] + offers() + TAKE_TURN + ACTIVE + probe(1))

S.case(130, 4, "bot.reboot re-enters live: no input is taken until the abilities offer again.",
       ACTIVE + [send("bot.reboot", js({"reason": "cognition_child_teardown_failed"}))] + TAKE_TURN + offers() + probe(1))
S.case(131, 4, "after a reboot the turn count goes on.",
       ACTIVE + [bot_input(), turn_event("bot.ability.failed", 1, message="done"),
                 send("bot.reboot", js({"reason": "cognition_child_teardown_failed"}))] + offers() + probe(2))

S.case(140, 5, "the cognition's output for the turn ends it: the next input is turn 2.",
       ACTIVE + [bot_input(), turn_event("bot.ability.cognition.output", 1, output=[])] + probe(2))
S.case(141, 5, "the cognition's failure for the turn ends it: the next input is turn 2.",
       ACTIVE + [bot_input(), turn_event("bot.ability.failed", 1, message="the decision failed (status)")] + probe(2))
S.case(142, 5, "an output for another turn is dropped: turn 1 is still in hand.",
       ACTIVE + [bot_input(), turn_event("bot.ability.cognition.output", 5, output=[]),
                 turn_event("bot.processing.timed_out", 1), turn_event("bot.processing.cancel.timed_out", 1), degraded(1)])
S.case(143, 5, "a bot.ability.cognition.input handoff starts a turn.",
       ACTIVE + [send("bot.ability.cognition.input", js({"stimulus": {"event": "bot.behavior.tick", "data": {"name": "Hourly"}, "source": "routines"}})),
                 turn_event("bot.processing.timed_out", 1), turn_event("bot.processing.cancel.timed_out", 1), degraded(1)])

S.case(150, 6, "an acknowledged cancel ends the turn: its cancel timer is no longer taken and the next input is turn 2.",
       ACTIVE + [bot_input(), turn_event("bot.processing.timed_out", 1), turn_event("bot.ability.cognition.cancelled", 1),
                 turn_event("bot.processing.cancel.timed_out", 1)] + probe(2))
S.case(151, 6, "bot.ability.processing.cancelled acknowledges the cancel too.",
       ACTIVE + [bot_input(), turn_event("bot.processing.timed_out", 1), turn_event("bot.ability.processing.cancelled", 1),
                 turn_event("bot.processing.cancel.timed_out", 1)] + probe(2))
S.case(152, 6, "a processing timer for another turn is not taken: the turn ends by its reply and the next input is turn 2.",
       ACTIVE + [bot_input(), turn_event("bot.processing.timed_out", 2), turn_event("bot.processing.cancel.timed_out", 1),
                 turn_event("bot.ability.failed", 1, message="late")] + probe(2))

S.case(160, 7, "an input during a turn waits and becomes turn 2 when the turn ends.",
       ACTIVE + [bot_input("first"), bot_input("second"), turn_event("bot.ability.failed", 1, message="done"),
                 turn_event("bot.processing.timed_out", 2), turn_event("bot.processing.cancel.timed_out", 2), degraded(2)])

S.case(170, 8, "an ability that ends while the bot is active degrades it.",
       ACTIVE + [send("done.invoke.listening", js({})),
                 expect(DONE, "/^the ability .* ended while active$/.test(_event.data.reason)")])
