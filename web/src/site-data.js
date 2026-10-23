const GH='https://github.com/stateforward/floppy/';
const SX='https://github.com/stateforward/scxml.rs/namespaces/';
const P=[
 {id:'sql',kind:'service',uri:GH+'services/sql',ver:'0.1.0',
  sum:'One SQLite transaction of statements per <invoke>. The engine under the Memory ability, with no schema of its own.',
  params:['database','statements'],events:['sql.result','sql.failed'],env:[],origins:[],grant:'--dir …/services/sql=HOST::/memory',
  artifact:'floppy_service_sql.virtualized.wasm',wit:'stateforward:scxml/invoked-service 0.2.0',used:['abilities/memory','abilities/migration'],
  snip:`<invoke id="store" type="${GH}services/sql">
  <param name="database" expr="database"/>
  <param name="statements" expr="statements"/>
</invoke>`},
 {id:'pgsql',kind:'service',uri:GH+'services/pgsql',ver:'0.1.0',
  sum:'One Postgres transaction per <invoke> over the wire protocol on wasi:sockets/tcp. Answers exactly as sql does.',
  params:['database','statements'],events:['sql.result','sql.failed'],env:[],origins:[],grant:'--wasi …/services/pgsql=inherit-network',
  artifact:'floppy_service_pgsql.virtualized.wasm',wit:'stateforward:scxml/invoked-service 0.2.0',used:['abilities/memory','abilities/migration'],
  snip:`<invoke id="store" type="${GH}services/pgsql">
  <param name="database" expr="'postgresql://floppy@10.0.0.4:5432/floppy'"/>
  <param name="statements" expr="statements"/>
</invoke>`},
 {id:'generate',kind:'service',uri:GH+'services/generate',ver:'0.1.0',
  sum:'One model generation per <invoke> on an OpenAI-compatible Chat Completions API, with one required function tool, as the original asks, and images by reference.',
  params:['model','system','prompt','messages','schema','schemaname'],events:['generate.result','generate.failed'],env:['apikey','baseurl'],origins:[],grant:'--dir …/services/generate=$PWD/media::/media',
  artifact:'floppy_service_generate.virtualized.wasm',wit:'stateforward:scxml/invoked-service 0.2.0',used:['abilities/reading','abilities/reasoning','abilities/reflection','bots/builder'],
  snip:`<invoke id="read" type="${GH}services/generate"
        env:apikey="FLOPPY_API_KEY" env:baseurl="FLOPPY_BASE_URL">
  <param name="model" expr="model"/>
  <param name="messages" expr="messages"/>
</invoke>`},
 {id:'speech',kind:'service',uri:GH+'services/speech',ver:'0.1.0',
  sum:'One speech or voice operation per <invoke>: decode, encode, detect, diarize, identify, and PCM steps in a granted directory.',
  params:['operation','model','audio','text','provider'],events:['bot.ability.hearing.speech.decoding.output','bot.ability.vocal.speech.encoding.output','bot.ability.failed'],env:['apikey','baseurl'],origins:[],grant:'--dir …/services/speech=$PWD/media::/media',
  artifact:'floppy_service_speech.virtualized.wasm',wit:'stateforward:scxml/invoked-service 0.2.0',used:['abilities/interpretation','abilities/speech_decoding','abilities/vocal','abilities/voice_detection','abilities/voice_diarization','abilities/voice_identification'],
  snip:`<invoke id="decoder" type="${GH}services/speech"
        env:apikey="FLOPPY_SPEECH_API_KEY" env:baseurl="FLOPPY_SPEECH_BASE_URL">
  <param name="operation" expr="'decode'"/>
  <param name="model" expr="'whisper-1'"/>
  <param name="audio" expr="'media/hello.wav'"/>
</invoke>`},
 {id:'decision',kind:'service',uri:GH+'services/decision',ver:'0.1.0',
  sum:'Ranks and classifies a stimulus against offered choices on TypeSafe, or abstains under the threshold.',
  params:['choices','state','data','instructions','threshold'],events:['decision.ranked','decision.classified','decision.abstain','decision.failed'],env:['apikey','baseurl'],origins:['https://api.typesafe.ai'],grant:'none',
  artifact:'floppy_service_decision.virtualized.wasm',wit:'stateforward:scxml/invoked-service 0.2.0',used:['abilities/intuition','bots/builder'],
  snip:`<invoke idlocation="decision_id" type="${GH}services/decision"
        env:apikey="TYPESAFE_API_KEY" env:baseurl="TYPESAFE_BASE_URL">
  <param name="choices" expr="choices"/>
  <param name="threshold" expr="threshold"/>
</invoke>`},
 {id:'decision',kind:'namespace',prefix:'decision',uri:GH+'namespaces/decision',ver:'0.1.0',
  sum:'The synchronous half of a decision: reads the choices a state offers and pins one property of them.',
  elements:['choice','pin'],artifact:'floppy_decision.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',
  used:['abilities/cognition','abilities/communication','abilities/identity','abilities/reflection','bots/builder','bots/floppy']},
 {id:'processing',kind:'namespace',prefix:'processing',uri:GH+'namespaces/processing',ver:'0.1.0',
  sum:'The original\u2019s processing module: model-facing schemas, the turn\u2019s offered events tagged with their sessions, the dispatch tool, the selection confidence. Never sends.',
  elements:['model_facing_schema','collect_offered_events','send','selection_confidence'],artifact:'floppy_processing.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',
  used:['abilities/cognition','abilities/communication','abilities/identity','abilities/intuition','abilities/reasoning','abilities/reflection','bots/floppy']},
 {id:'intuition',kind:'namespace',prefix:'intuition',uri:GH+'namespaces/intuition',ver:'0.1.0',
  sum:'The original\u2019s intuition ConfidenceTuner: observes each turn\u2019s confidence and escalates when it is below the tuned threshold. Never sends.',
  elements:['observe','escalate'],artifact:'floppy_intuition.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',
  used:['abilities/intuition']},
 {id:'cognition',kind:'namespace',prefix:'cognition',uri:GH+'namespaces/cognition',ver:'0.1.0',
  sum:'The original\u2019s cognition module: the turn\u2019s attention bias and the model-facing projection of a turn\u2019s input. Never sends.',
  elements:['attention_bias_for_turn','model_facing_content'],artifact:'floppy_cognition.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',
  used:['abilities/reasoning','bots/floppy']},
 {id:'conversation',kind:'namespace',prefix:'conversation',uri:GH+'namespaces/conversation',ver:'0.1.0',
  sum:'The original\u2019s conversation module and its turn detector: committed messages, relationships and participants, detector stimuli and closed turns. Never holds the history and never sends.',
  elements:['message_for_turn','relationship_for_input','participant_for_source','fail_turn','detector_refs','new_track_ref','content_for_detector','complete_turn','ingest_content','on_end','turn_participants'],artifact:'floppy_conversation.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',used:['abilities/conversation','abilities/speaking','abilities/turn_detector']},
 {id:'identity',kind:'namespace',prefix:'identity',uri:GH+'namespaces/identity',ver:'0.1.0',
  sum:'The original\u2019s identity module: whether a text names the bot, and the canonical order of participant identities. Never sends.',
  elements:['hears_name','sorted_identities'],artifact:'floppy_identity.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',used:['abilities/conversation','abilities/identity']},
 {id:'memory',kind:'namespace',prefix:'memory',uri:GH+'namespaces/memory',ver:'0.1.0',
  sum:'The original\u2019s memory module: a conversation memory row\u2019s JSON and the contents of a memory select. Never sends, never touches the database.',
  elements:['to_json','contents'],artifact:'floppy_memory.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',used:['abilities/conversation','abilities/reasoning']},
 {id:'behavior',kind:'namespace',prefix:'behavior',uri:GH+'namespaces/behavior',ver:'0.1.0',
  sum:'The behavior package: inventory rows read as behaviors, the statements that replace one, a lifetime\u2019s schedule bounds, the check of a routine\u2019s forced tick, the check of a turn behavior\u2019s run on its live turn, and a turn behavior\u2019s one program shape. Never sends, never touches the database.',
  elements:['instances_from_behavior_results','replace_behavior_clauses','lifetime_errors','verify_tick','verify_apply','turn_program'],artifact:'floppy_behavior.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',used:['abilities/reflection','bots/builder']},
 {id:'autonomy',kind:'namespace',prefix:'autonomy',uri:GH+'namespaces/autonomy',ver:'0.1.0',
  sum:'The original\u2019s autonomy module: the input a learned behavior runs with, the stimulus data with the turn\u2019s focus. Never sends.',
  elements:['behavior_input_payload'],artifact:'floppy_autonomy.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',used:['abilities/autonomy','abilities/reflection']},
 {id:'migration',kind:'namespace',prefix:'migration',uri:GH+'namespaces/migration',ver:'0.1.0',
  sum:'Alembic\u2019s revision graph walk, agnostic of the database. Knows revisions, never SQL.',
  elements:['plan'],artifact:'floppy_migration.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',used:['abilities/migration']},
 {id:'event',kind:'namespace',prefix:'event',uri:SX+'event',ver:'built in',pub:'stateforward/scxml.rs',
  sum:'Built into scx. event:schema checks an event\u2019s data against a JSON Schema before a transition is enabled.',
  elements:['schema (attribute)'],artifact:'embedded in scx',wit:'n/a',used:['28 documents']},
 {id:'scx',kind:'namespace',prefix:'scx',uri:SX+'scx',ver:'built in',pub:'stateforward/scxml.rs',
  sum:'Built into scx. scx:validate checks a bot definition from inside a running machine.',
  elements:['validate'],artifact:'embedded in scx',wit:'n/a',used:['bots/builder']},
];
const EL=[
 ['decision','choice','location events',6,'Offers events as enabled transitions the turn can select.'],
 ['decision','pin','location event property values',1,'Pins an event property to an enum of allowed values.'],
 ['processing','model_facing_schema','location bound? overlocation?',4,'The model-facing choices: no $schema, title or examples, and over the bound no descriptions.'],
 ['processing','collect_offered_events','location choices offers? devices? focus? bound? overlocation? sizelocation?',1,'The turn\u2019s choices, each tagged with its offering session\u2019s id.'],
 ['processing','send','location choices max?',2,'The original\u2019s dispatch tool schema.'],
 ['processing','selection_confidence','location selections',1,'The least normalized confidence of the selections.'],
 ['intuition','observe','location confidence',1,'Observes a turn\u2019s confidence into the tuner state.'],
 ['intuition','escalate','location confidence tuner',1,'Whether the confidence is below the tuner\u2019s threshold.'],
 ['cognition','attention_bias_for_turn','location focuslocation? stimulus devices focus',1,'The turn\u2019s attention bias: focus and focus candidates.'],
 ['cognition','model_facing_content','location failurelocation? stimulus prior_episodes standing_directives environment? instructions? schema?',1,'The model-facing XML of a turn\u2019s input, under the original\u2019s budgets.'],
 ['identity','hears_name','location name text',1,'Whether the text names the bot.'],
 ['identity','sorted_identities','location ids',1,'The identities in the original\u2019s order.'],
 ['memory','to_json','location message',1,'A conversation memory row\u2019s JSON, as the original writes it.'],
 ['memory','contents','location result',1,'The JSON objects of one statement\u2019s result, in row order.'],
 ['conversation','message_for_turn','location direction event message provenance?',2,'Builds one committed conversation message.'],
 ['conversation','relationship_for_input','location indexlocation input',1,'The relationship an input belongs to, a new one appended.'],
 ['conversation','participant_for_source','location source',1,'Makes the source a participant of the relationship.'],
 ['conversation','fail_turn','location source',1,'Drops the participant whose turn failed.'],
 ['conversation','detector_refs','location relationships',1,'Every participant\u2019s (relationship, source) pair.'],
 ['conversation','new_track_ref','location source',1,'A participant\u2019s track.'],
 ['conversation','content_for_detector','location input source',1,'The stimulus that opens a source\u2019s turn.'],
 ['conversation','complete_turn','location event input turns session_ref participants?',1,'The inbound message the turns make of the input.'],
 ['conversation','ingest_content','location content',1,'Takes a stimulus into the open turn\u2019s content.'],
 ['conversation','on_end','location turn participant_ref conversation_ref turn_ref source_participant_ref',1,'The closed turn, or why it failed.'],
 ['conversation','turn_participants','location input parent?',1,'The participants a turn\u2019s cognition handoff stamps.'],
 ['behavior','instances_from_behavior_results','location behaviors triggers',1,'The behavior instances of a memory select, with their triggers.'],
 ['behavior','replace_behavior_clauses','location instance',1,'The statements that replace one behavior\u2019s row and triggers.'],
 ['behavior','lifetime_errors','location scxml min_every? turn_after_bound? lifetimelocation? scheduleslocation?',2,'The schedule bounds a behavior\u2019s lifetime implies (E0009).'],
 ['behavior','verify_tick','location ticks',1,'The check of a persistent routine\u2019s forced tick (E0008).'],
 ['behavior','verify_apply','location done turn input targets handled? error?',1,'The check of a turn behavior\u2019s run on its live turn or a test input (E0008).'],
 ['behavior','turn_program','location script',1,'The turn behavior template naming the behavior\u2019s script.'],
 ['autonomy','behavior_input_payload','location stimulus focus focus_candidates',2,'The input a learned behavior runs with.'],
 ['migration','plan','location revisions current target?',1,'Orders migration steps from current to target.'],
 ['scx','validate','',1,'Validates a document from inside a machine.'],
].map(([ns,n,attrs,uses,sum])=>({id:ns+':'+n,kind:'element',ns,n,attrs:attrs?attrs.split(' '):[],uses,sum,
  uri:(ns==='scx'?SX:GH+'namespaces/')+ns+'#'+n,ver:ns==='scx'?'built in':'0.1.0',
  snip:`<scxml xmlns:${ns}="${ns==='scx'?SX+'scx':GH+'namespaces/'+ns}" …>
  …
  <${ns}:${n}${attrs?' '+attrs.split(' ').filter(a=>!a.endsWith('?')).slice(0,3).map(a=>a+'="…"').join(' '):''}/>`}));
const ALL=[...P.filter(p=>p.kind==='service'),...P.filter(p=>p.kind==='namespace'),...EL];
