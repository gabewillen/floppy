// The ECMAScript of abilities/seeing/seeing.scx (SCXML 5.8): loaded by its root
// <script src="seeing.js">, so every function here is a global of the
// document's data model, callable from its cond and expr attributes.

// Shared: the original's MAX_VIEWS and participants bound, send delays, the
// timer in hand, and a view's freshness.

var MAX_VIEWS = 4;
var MAX_PARTICIPANTS = 32;

function delay(ms) {
  return ms + 'ms';
}

function isTimer(_event) {
  return _event.sendid === timer;
}

// How long ago a view arrived at now, never negative (the original _age).
function ageMs(view, now) {
  return Math.max(0, now - view.received_at);
}

function isFresh(view, now) {
  return ageMs(view, now) <= max_view_age_ms;
}

// perceiving: holding (the original _hold)

// The views and outcomes once _event's view is held: stale views expire;
// a view without a source is dropped; else it replaces its source's view
// (latest wins), moved to the end (views is in receipt order), unmeasured,
// keeping the source's measured_at; past MAX_VIEWS the least recently
// received view is evicted.
function hold(_event) {
  var now = Date.now();
  var outcomes = [];
  var forgotten = [];
  var kept = views.filter(function (view) {
    var fresh = isFresh(view, now);
    if (!fresh) {
      outcomes.push('expired');
      forgotten.push(view.source);
    }
    return fresh;
  });
  var source = _event.data.source;
  if (typeof source !== 'string') {
    outcomes.push('dropped');
    return {views: kept, outcomes: outcomes, forgotten: forgotten};
  }
  var previous = kept.filter(function (view) { return view.source === source; })[0];
  kept = kept.filter(function (view) { return view.source !== source; });
  kept.push({source: source, visual: _event.data, received_at: now, measured: false, measured_at: previous === undefined ? null : previous.measured_at});
  outcomes.push('held');
  while (kept.length > MAX_VIEWS) {
    forgotten.push(kept.shift().source);
    outcomes.push('evicted');
  }
  return {views: kept, outcomes: outcomes, forgotten: forgotten};
}

// The seen signatures without the sources a hold forgot: an expired or
// evicted source's signature goes with it (the original _forget_source), so
// its next view is a first view again, even when that view is the one just
// held.
function seenWithout(forgotten) {
  return seen.filter(function (entry) { return forgotten.indexOf(entry.source) === -1; });
}

// next

// The time of the choice: next reads the clock once, and its choice, the
// look's request and the measured pick all see the views as of that time.
function clockNow() {
  return Date.now();
}

function hasUnmeasuredView() {
  return views.some(function (view) { return !view.measured && isFresh(view, chosen_at); });
}

// A view as the eye hands it on (the original's View): the stimulus's
// image, media type, kind and participants, its change and age; no stage
// has filled objects or description yet.
function handedView(visual, change, age) {
  return {image: visual.image, media_type: orNull(visual.media_type), kind: orNull(visual.kind), participants: visual.participants || [], change: change, age_ms: age, objects: null, description: null};
}

function orNull(value) {
  return value === undefined ? null : value;
}

// The look's request (the original _take_look): every fresh view, newest
// first (a stable sort, so views that arrived together stay in receipt
// order), change null, from Seeing itself.
function lookRequest() {
  var now = chosen_at;
  var fresh = views.filter(function (view) { return isFresh(view, now); });
  fresh.sort(function (a, b) { return b.received_at - a.received_at; });
  return {cause: 'look', source: 'seeing', views: fresh.map(function (view) { return handedView(view.visual, null, ageMs(view, now)); })};
}

// measuring (the original _measure)

// The unmeasured fresh view of the least recently measured source: never
// measured first, then the oldest measured_at, ties to the oldest arrival.
// next chose measuring at chosen_at because such a view was fresh then, so
// there is one; null only if there were none.
function measurePick() {
  var now = chosen_at;
  var candidates = views.filter(function (view) { return !view.measured && isFresh(view, now); });
  var key = function (view) {
    return [view.measured_at === null ? 0 : 1, view.measured_at === null ? now : view.measured_at, view.received_at];
  };
  var picked = candidates.reduce(function (best, view) {
    if (best === null) {
      return view;
    }
    var a = key(view);
    var b = key(best);
    for (var i = 0; i < a.length; i += 1) {
      if (a[i] !== b[i]) {
        return a[i] < b[i] ? view : best;
      }
    }
    return best;
  }, null);
  if (picked === null) {
    return null;
  }
  return {source: picked.source, visual: picked.visual, received_at: picked.received_at};
}

// The views with the picked source's view marked measured at the choice.
function markedMeasured() {
  return views.map(function (view) {
    return measured !== null && view.source === measured.source ? Object.assign({}, view, {measured: true, measured_at: chosen_at}) : view;
  });
}

// The measured view's image, or null with no view picked (the vision
// service then fails the measure).
function measuredImage() {
  return measured === null ? null : measured.visual.image;
}

// The signature last handed on for the measured source, or null.
function previousSignature() {
  var entry = seen.filter(function (entry) { return measured !== null && entry.source === measured.source; })[0];
  return entry === undefined ? null : entry.signature;
}

// The vision service's done data is the original's Measurement.
function isMeasurement(_event) {
  var data = Object(_event.data);
  return (data.change === null || typeof data.change === 'number') && typeof Object(data.signature).measure === 'string';
}

function measureCouldNotRunMessage(_event) {
  return 'the vision service could not measure the view (' + _event.name + ')';
}

function measureTimedOutMessage() {
  return 'the measurement timed out after ' + measure_timeout_ms + ' ms';
}

// The original _is_salient: a first view, or a change at the threshold or past it.
function isSalient(_event) {
  var change = _event.data.measurement.change;
  return change === null || change >= change_threshold;
}

function salience(_event) {
  return _event.data.measurement.change === null ? 'first' : 'salient';
}

// The original _take_change: the measured signature becomes what later views
// of its source are measured against, while the source is still held.
function seenAfter(_event) {
  var source = _event.data.source;
  if (!views.some(function (view) { return view.source === source; })) {
    return seen;
  }
  var kept = seen.filter(function (entry) { return entry.source !== source; });
  kept.push({source: source, signature: _event.data.measurement.signature});
  return kept;
}

// The change's request: the one measured view, from its source.
function changeRequest(_event) {
  var view = handedView(_event.data.visual, _event.data.measurement.change, ageMs(_event.data, Date.now()));
  return {cause: 'change', source: _event.data.source, views: [view]};
}

function measureFailedOutcome(_event) {
  return _event.data.kind === 'timeout' ? 'timeout' : 'failed';
}

// interpreting

function isViewLeft() {
  return view_index < request.views.length;
}

function currentView() {
  return request.views[view_index];
}

function nextViewIndex() {
  return view_index + 1;
}

// The sight's participants (the original _merged_participants without
// faces): the views' stamped participants, first occurrence first, bounded.
function sightParticipants() {
  var merged = [];
  request.views.forEach(function (view) {
    view.participants.forEach(function (participant) {
      var known = merged.some(function (other) { return other.ref === participant.ref && other.role === participant.role; });
      if (!known) {
        merged.push(participant);
      }
    });
  });
  return merged.slice(0, MAX_PARTICIPANTS);
}

// The sight's envelope: the view's source for a change, Seeing for a look.
function sightStimulus(_event) {
  return {event: 'bot.ability.seeing.sight', data: _event.data, source: request.source};
}
