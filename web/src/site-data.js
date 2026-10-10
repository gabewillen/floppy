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
  sum:'One model generation per <invoke> on an OpenAI-compatible Chat Completions API, with strict structured output and images by reference.',
  params:['model','system','prompt','messages','schema','schemaname','strict'],events:['generate.result','generate.failed'],env:['apikey','baseurl'],origins:[],grant:'--dir …/services/generate=$PWD/media::/media',
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
  sum:'The canonical extension namespace: builds, compacts, pins and routes the choices a turn offers.',
  elements:['choice','selection','names','assemble','compact','pin','route','tuner'],artifact:'floppy_decision.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',
  sha:'59f0b7a03fe7…ba01',used:['abilities/cognition','abilities/communication','abilities/identity','abilities/intuition','abilities/reasoning','abilities/reflection','bots/builder','bots/floppy']},
 {id:'bot',kind:'namespace',prefix:'bot',uri:GH+'namespaces/bot',ver:'0.1.0',
  sum:'A bot body\u2019s synchronous per-turn bookkeeping. Reads expressions, assigns results, never sends, does no I/O.',
  elements:['attention','stimulus','recognize'],artifact:'floppy_bot.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',
  used:['abilities/cognition','abilities/conversation','abilities/identity','abilities/listening','bots/floppy']},
 {id:'conversation',kind:'namespace',prefix:'conversation',uri:GH+'namespaces/conversation',ver:'0.1.0',
  sum:'One committed conversation message. Never holds the history and never crosses the extension boundary with it.',
  elements:['commit'],artifact:'floppy_conversation.virtualized.wasm',wit:'stateforward:scxml/executable-content-extension 0.2.0',used:['abilities/conversation','abilities/speaking']},
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
 ['decision','selection','location choices',2,'Builds the strict structured-output schema from the choices.'],
 ['decision','names','location choices',5,'Writes the logged choice names.'],
 ['decision','assemble','location routeslocation choices offers bound overlocation sizelocation',1,'Appends each ability\u2019s latest offers, tagged with its invoke id.'],
 ['decision','compact','location bound overlocation',4,'Drops $schema, title and examples to fit the bound.'],
 ['decision','pin','location event property values',1,'Pins an event property to an enum of allowed values.'],
 ['decision','route','location selections routes',1,'Places each selected event with its receiver.'],
 ['decision','tuner','location thresholdlocation observe',1,'Tunes the abstain threshold from observed outcomes.'],
 ['bot','attention','location focuslocation? stimulus devices focus sources?',1,'The turn\u2019s attention bias: focus and focus candidates.'],
 ['bot','stimulus','location event data source target? id?',5,'The cognition input envelope {event, data, source, target?, id?}.'],
 ['bot','recognize','location name text',1,'Whether the text names the bot.'],
 ['conversation','commit','location keylocation? direction event message sequence provenance?',2,'Builds one committed conversation message.'],
 ['migration','plan','location revisions current target?',1,'Orders migration steps from current to target.'],
 ['scx','validate','',1,'Validates a document from inside a machine.'],
].map(([ns,n,attrs,uses,sum])=>({id:ns+':'+n,kind:'element',ns,n,attrs:attrs?attrs.split(' '):[],uses,sum,
  uri:(ns==='scx'?SX:GH+'namespaces/')+ns+'#'+n,ver:ns==='scx'?'built in':'0.1.0',
  snip:`<scxml xmlns:${ns}="${ns==='scx'?SX+'scx':GH+'namespaces/'+ns}" …>
  …
  <${ns}:${n}${attrs?' '+attrs.split(' ').filter(a=>!a.endsWith('?')).slice(0,3).map(a=>a+'="…"').join(' '):''}/>`}));
const ALL=[...P.filter(p=>p.kind==='service'),...P.filter(p=>p.kind==='namespace'),...EL];
