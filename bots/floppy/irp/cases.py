# The floppy body's statements and cases (tests/irp/harness.py writes them).
#
# The harness is the body's environment: it invokes the body, which starts
# in live and activates on its own (decision D107), and receives every
# event the body emits (decision D102), which a case expects in order. The
# body's real abilities answer too, so a case ignores their products the
# body passes on (bot.ability.*), and an activation it must control waits
# for an extra expected ability, "ghost", that only the case offers for
# (GHOST). Every event name the body takes has one meaning whoever sends
# it, so a case sends the abilities' replies and the body's own timers
# itself, in one burst the body takes before its real abilities answer:
#
#   probe(k): a bot.input, then bot.processing.timed_out and
#     bot.processing.cancel.timed_out for turn k: when the bot is active and
#     the input is turn k, the body reports the timeout, the unacknowledged
#     cancel and the turn's failure, and ends in degraded with "turn k: the
#     cognition did not acknowledge its cancel"; otherwise the probe is not
#     taken and the case waits (and fails).
#   TAKE_TURN: a bot.input answered at once by bot.ability.failed for turn
#     1: when the bot is active it uses up turn 1 (and reports
#     bot.processing.failed, which fails the case); when it is not, nothing
#     happens.

S = suite
DEACTIVATE = "bot.deactivate"
OFFERS = "bot.ability.offers"
INPUT = "bot.input"
DONE = "done.invoke.floppy"
ABILITIES = ["cognition", "communication", "identity", "listening"]
GHOST = ("expected", js(ABILITIES + ["ghost"]))
# The abilities' products the body passes on to its environment.
NOISE = ("bot.ability",)


def offers(*abilities):
    return [send(OFFERS, js({"ability": a, "choices": []})) for a in (abilities or ABILITIES)]


def bot_input(text="hello"):
    return send(INPUT, js({"observation": {"event": "text", "data": text}}))


def turn_event(event, k, **extra):
    return send(event, js(dict({"turn": k}, **extra)))


def for_turn(event, k):
    return expect(event, f"_event.data.turn === {k}")


def degraded(k):
    return [for_turn("bot.processing.timed_out", k), for_turn("bot.processing.cancel.timed_out", k),
            for_turn("bot.processing.failed", k), expect("bot.degraded", "_event.data.stage === 'processing_cancel'"),
            expect(DONE, f"_event.data.reason === {js(f'turn {k}: the cognition did not acknowledge its cancel')}")]


def probe(k):
    return [bot_input("probe"), turn_event("bot.processing.timed_out", k), turn_event("bot.processing.cancel.timed_out", k)] + degraded(k)


TAKE_TURN = [bot_input("taken"), turn_event("bot.ability.failed", 1, message="taken")]
ACTIVATED = expect("bot.activated")
ACTIVE = [ACTIVATED]
LEFT = expect(DONE)


# The harness is the body's environment: its _sessionid is the body's
# environment param (decision D106).
ENVIRONMENT = ("environment", "_sessionid")


def case(test_id, number, comment, steps, params=(), **options):
    S.case(test_id, number, comment, steps, ignore=NOISE, params=[ENVIRONMENT] + list(params), **options)


S.statement(1, "Invoked, the body starts in `live`, which invokes the abilities (`cognition`, `communication`, `identity`, `listening`), and activates on its own (no `bot.activate`): it sends `bot.activated` to its parent, and takes a turn's input, only once every expected ability (the data `expected`) has sent its first `bot.ability.offers`.")
S.statement(2, "While `activating`, an `error.*` event, an ability's `done.invoke`, or the activation timer (`activation_timeout_ms`) fails the activation: the body sends `bot.activating.failed` to its parent and its session ends in the final state `inactive`.")
S.statement(3, "`bot.deactivate` leaves `live` and ends the session in `inactive`: from `active` or `activating`, after sending `bot.deactivated` to its parent (every accepted request is answered).")
S.statement(4, "`bot.reboot` `{reason}` re-enters `live`, and the body passes it on to its parent: the abilities are invoked again and the bot waits for their offers again (and sends `bot.activated` again) before it takes input; the turn count goes on.")
S.statement(5, "In `unfocused` or `focused`, `bot.input` or a `bot.ability.cognition.input` `{stimulus}` handoff starts a `processing` turn, counted from 1; the cognition's `bot.ability.cognition.output` or `bot.ability.failed` for the current turn ends it (back to attention) with `bot.processing.completed` or `bot.processing.failed` to the parent, and one for another turn is dropped.")
S.statement(6, "`bot.processing.timed_out` for the current turn moves to `cancelling_processing` and is passed on to the parent; the acknowledgement (`bot.ability.cognition.cancelled` or `bot.ability.processing.cancelled` for the turn) ends the turn with `bot.processing.failed`; `bot.processing.cancel.timed_out` for it degrades the bot: it sends `bot.processing.cancel.timed_out`, `bot.processing.failed` and `bot.degraded` `{stage: \"processing_cancel\"}`, and the session ends in the final state `degraded` with `{reason: \"turn <n>: the cognition did not acknowledge its cancel\"}`. A timer for another turn is not taken.")
S.statement(7, "An input that arrives during a turn waits and starts the next turn when the bot returns to attention.")
S.statement(8, "An ability that ends while the bot is active (`done.invoke.<id>`) degrades it: the session ends in `degraded` with `{reason: \"the ability <id> ended while active\"}`.")

case(100, 1, "activated by every ability's offers, the bot takes a turn: probe(1) degrades it at turn 1.",
     ACTIVE + probe(1))
case(101, 1, "with an expected ability that has not offered, the bot takes no input; once it offers, it does.",
     TAKE_TURN + offers("ghost") + [ACTIVATED] + probe(1),
     params=[GHOST])

case(110, 2, "an error while activating fails the activation and ends the session in inactive.",
     [send("error.platform", js({})), expect("bot.activating.failed"), LEFT],
     params=[GHOST])
case(111, 2, "an ability ending while activating fails the activation.",
     [send("done.invoke.listening", js({})), expect("bot.activating.failed"), LEFT],
     params=[GHOST])
case(112, 2, "an activation that times out (an expected ability never offers) fails it.",
     [expect("bot.activating.failed"), LEFT],
     params=[GHOST, ("activation_timeout_ms", "100")])

case(120, 3, "bot.deactivate from active reports bot.deactivated and ends the session.",
     ACTIVE + [send(DEACTIVATE, js({})), expect("bot.deactivated"), LEFT])
case(121, 3, "bot.deactivate while activating answers bot.deactivated and ends the session.",
     [send(DEACTIVATE, js({})), expect("bot.deactivated"), LEFT],
     params=[GHOST])

case(130, 4, "bot.reboot re-enters live: no input is taken until the abilities offer again.",
     offers("ghost") + [ACTIVATED, send("bot.reboot", js({"reason": "cognition_child_teardown_failed"})), expect("bot.reboot")]
     + TAKE_TURN + offers("ghost") + [ACTIVATED] + probe(1),
     params=[GHOST])
case(131, 4, "after a reboot the turn count goes on.",
     ACTIVE + [bot_input(), turn_event("bot.ability.failed", 1, message="done"), for_turn("bot.processing.failed", 1),
               send("bot.reboot", js({"reason": "cognition_child_teardown_failed"})), expect("bot.reboot"), ACTIVATED]
     + probe(2))

case(140, 5, "the cognition's output for the turn ends it: the next input is turn 2.",
     ACTIVE + [bot_input(), turn_event("bot.ability.cognition.output", 1, output=[]), for_turn("bot.processing.completed", 1)]
     + probe(2))
case(141, 5, "the cognition's failure for the turn ends it: the next input is turn 2.",
     ACTIVE + [bot_input(), turn_event("bot.ability.failed", 1, message="the decision failed (status)"),
               for_turn("bot.processing.failed", 1)] + probe(2))
case(142, 5, "an output for another turn is dropped: turn 1 is still in hand.",
     ACTIVE + [bot_input(), turn_event("bot.ability.cognition.output", 5, output=[]),
               turn_event("bot.processing.timed_out", 1), turn_event("bot.processing.cancel.timed_out", 1)] + degraded(1))
case(143, 5, "a bot.ability.cognition.input handoff starts a turn.",
     ACTIVE + [send("bot.ability.cognition.input", js({"stimulus": {"event": "bot.behavior.tick", "data": {"name": "Hourly"}, "source": "routines"}})),
               turn_event("bot.processing.timed_out", 1), turn_event("bot.processing.cancel.timed_out", 1)] + degraded(1))

case(150, 6, "an acknowledged cancel ends the turn: its cancel timer is no longer taken and the next input is turn 2.",
     ACTIVE + [bot_input(), turn_event("bot.processing.timed_out", 1), turn_event("bot.ability.cognition.cancelled", 1),
               turn_event("bot.processing.cancel.timed_out", 1), for_turn("bot.processing.timed_out", 1),
               for_turn("bot.processing.failed", 1)] + probe(2))
case(151, 6, "bot.ability.processing.cancelled acknowledges the cancel too.",
     ACTIVE + [bot_input(), turn_event("bot.processing.timed_out", 1), turn_event("bot.ability.processing.cancelled", 1),
               turn_event("bot.processing.cancel.timed_out", 1), for_turn("bot.processing.timed_out", 1),
               for_turn("bot.processing.failed", 1)] + probe(2))
case(152, 6, "a processing timer for another turn is not taken: the turn ends by its reply and the next input is turn 2.",
     ACTIVE + [bot_input(), turn_event("bot.processing.timed_out", 2), turn_event("bot.processing.cancel.timed_out", 1),
               turn_event("bot.ability.failed", 1, message="late"), for_turn("bot.processing.failed", 1)] + probe(2))

case(160, 7, "an input during a turn waits and becomes turn 2 when the turn ends.",
     ACTIVE + [bot_input("first"), bot_input("second"), turn_event("bot.ability.failed", 1, message="done"),
               turn_event("bot.processing.timed_out", 2), turn_event("bot.processing.cancel.timed_out", 2),
               for_turn("bot.processing.failed", 1)] + degraded(2))

case(170, 8, "an ability that ends while the bot is active degrades it.",
     ACTIVE + [send("done.invoke.listening", js({})),
               expect(DONE, "/^the ability .* ended while active$/.test(_event.data.reason)")])
