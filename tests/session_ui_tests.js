// Save/Discard/Cancel state transitions and Finder-open regression coverage.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
function harness(){
  const nodes=new Map(),calls=[],handlers=new Map(),timers=new Map();let context,clock=0,nextTimer=1;
  function advance(milliseconds){clock+=milliseconds;for(const [id,timer] of [...timers])if(timer.at<=clock){timers.delete(id);timer.fn();}}
  function element(id){if(!nodes.has(id))nodes.set(id,{id,innerHTML:'',value:'',dataset:{},attributes:{},classList:{toggle(){},add(){},remove(){}},focus(){},select(){},scrollIntoView(){},closest(){return null;},querySelectorAll(){return[];},addEventListener(){},setAttribute(name,value){this.attributes[name]=String(value);},click(){return this.onclick?.();}});return nodes.get(id);}
  const document={body:element('body'),activeElement:null,querySelector:element,querySelectorAll:()=>[],getElementById:element,addEventListener(){},createElement:element};
  const window={addEventListener(){},STHostTransport:hostStub({post(message){calls.push(message);const handler=handlers.get(message.command);Promise.resolve().then(()=>handler?handler(message.payload):{}).then(result=>window.STHostTransport.deliver({id:message.id,ok:true,result}),error=>window.STHostTransport.deliver({id:message.id,ok:false,error:error.message}));}})};
  context=vm.createContext({window,document,console,setTimeout:(fn,delay=0)=>{const id=nextTimer++;timers.set(id,{fn,at:clock+delay});return id;},clearTimeout:id=>timers.delete(id),Date,Map,Set,Math,JSON,Number,String,Object,Array,Promise,innerWidth:1280,innerHeight:800});
  web.load(context, 'app.js');
  web.pendingEditsStub(window, 'nksControls', 'STNKSControls', 30);
  const ST=window.ST;ST.state.project=ST.demoProject();ST.state.project.name='Unsaved piece';ST.state.dirty=true;
  // Readable channel identities: track 0 is the owner 'track:t0' in the plug-in fixtures below.
  ST.state.project.tracks.forEach((track,index)=>{track.routingID='t'+index;});
  handlers.set('project.new',()=>null);handlers.set('project.open',()=>({project:{name:'Opened piece'}}));handlers.set('project.importMIDI',()=>({project:{name:'Imported MIDI'}}));handlers.set('project.save',payload=>({saved:true,project:payload.project,path:'/tmp/piece.supertracker'}));
  return {ST,window,document,calls,handlers,element,advance,load(name){web.load(context, name);}};
}
async function tick(){for(let i=0;i<12;i++)await Promise.resolve();}
async function run(){
  {
    function assertBlank(project){
      assert.equal(project.name,'Untitled session');assert.equal(project.bpm,120);
      assert.equal(project.tracks.length,1);assert.equal(project.tracks[0].name,'Track 01');
      assert.equal(project.tracks[0].instrument,0);assert.equal(project.tracks[0].delaySend,0);assert.equal(project.tracks[0].reverbSend,0);
      assert.equal(project.instruments.length,1);assert.equal(project.instruments[0].name,'Instrument 01');
      assert.equal(project.instruments[0].type,'sample');assert.equal(project.instruments[0].waveform,undefined);assert.equal(project.instruments[0].samplePath,'');
      assert.equal(project.patterns.length,1);const pattern=project.patterns[0];
      assert.equal(pattern.length,64);assert.equal(pattern.steps.length,1);assert.equal(pattern.steps[0].length,64);
      assert.ok(pattern.steps[0].every(step=>step===null));assert.equal(pattern.automation.length,0);
      assert.equal(JSON.stringify(project.sequence),'[0]');assert.equal(project.audioPlugins.length,0);
      assert.deepEqual(JSON.parse(JSON.stringify(project.routing)),{buses:[],outputs:{},sends:[],sidechains:[]});
    }
    for(const native of [true,false]){
      const h=harness();h.ST.state.native=native;await h.ST.boot();assertBlank(h.ST.state.project);
      assert.equal(h.ST.state.dirty,false);
      if(native){const sent=h.calls.filter(x=>x.command==='engine.project');assert.ok(sent.length>0);assertBlank(sent.at(-1).payload.project);}
      const blank=h.ST.state.project;
      await h.ST.projectAction('demo');assert.equal(h.ST.state.project.tracks.length,8);assert.equal(h.ST.state.project.patterns.length,2);
      assert.ok(h.ST.state.project.patterns.some(pattern=>pattern.steps.some(lane=>lane.some(step=>step?.note>=0))));
      h.ST.state.dirty=false;await h.ST.projectAction('new');assertBlank(h.ST.state.project);assert.notEqual(h.ST.state.project,blank);
      assertBlank(blank);
    }
    console.log('PASS actual native and preview boot/New create independent empty one-lane sessions; explicit demo retains eight populated lanes');
    const preview=harness();preview.ST.state.native=false;
    const first=(await preview.ST.request('project.new')).project,second=(await preview.ST.request('project.new')).project;
    assertBlank(first);assertBlank(second);first.patterns[0].steps[0][0]={note:60};assertBlank(second);
    console.log('PASS preview project.new returns a fresh one-lane project with unshared steps');
    for(const source of ['recovery','open']){
      const h=harness(),saved=h.ST.demoProject();saved.name='Existing dimensions';
      saved.tracks=saved.tracks.slice(0,3);saved.patterns=saved.patterns.map(p=>({...p,length:23,steps:p.steps.slice(0,3).map(lane=>lane.slice(0,23))}));
      const expected=JSON.stringify(saved.patterns.map(p=>p.steps));
      if(source==='recovery'){h.handlers.set('ready',()=>({project:saved,dirty:true}));await h.ST.boot();assert.equal(h.ST.state.dirty,true);}
      else{h.ST.state.dirty=false;h.handlers.set('project.open',()=>({project:saved}));await h.ST.projectAction('open','/tmp/existing.supertracker');}
      const project=h.ST.state.project;assert.equal(project.name,'Existing dimensions');assert.equal(project.tracks.length,3);
      assert.equal(project.patterns.length,2);assert.ok(project.patterns.every(p=>p.length===23&&p.steps.length===3));
      assert.equal(JSON.stringify(project.patterns.map(p=>p.steps)),expected);assert.deepEqual(JSON.parse(JSON.stringify(project.routing)),{buses:[],outputs:{},sends:[],sidechains:[]},'A session without a routing table gets the empty one: there is one mix model');
    }
    console.log('PASS startup recovery and Open preserve saved lane counts, pattern lengths, and notes');
  }
  {
    function takesHarness(command){
      const h=harness();h.load('clip-takes-project.js');h.load('clip-takes.js');
      h.ST.state.project.audioClips=[{id:'take-clip',track:0,name:'Original',path:'/original.wav',startBeat:0,durationBeats:1,sourceOffset:0,sourceDuration:1}];
      h.ST.state.clipSelected='take-clip';h.ST.mutate('rename project',p=>p.name='Edited with take preview');
      if(command==='redo')h.ST.perform('undo');
      h.window.STClipTakes.show();assert.equal(h.ST.state.modal,'clip-takes');
      return h;
    }
    for(const command of ['undo','redo'])for(const route of ['app','native-menu'])for(const previewState of ['settled','pending']){
      const h=takesHarness(command),owner=h.ST.state.project;let finishPreview,closed=0;
      if(previewState==='pending')h.handlers.set('sample.preview',()=>new Promise(resolve=>finishPreview=resolve));
      h.element('takes-preview').click();await tick();
      assert.equal(h.calls.filter(x=>x.command==='sample.preview').length,1);
      const originalClose=h.window.STClipTakes.closed;
      h.window.STClipTakes.closed=()=>{assert.equal(h.ST.state.project,owner,'preview must stop before project identity changes');closed++;originalClose();assert.equal(h.calls.filter(x=>x.command==='sample.stopPreview').length,1);};
      if(route==='app')await h.ST.perform(command);else h.window.nativeEvent({type:'command',payload:{name:command}});
      await tick();assert.equal(closed,1);assert.equal(h.ST.state.modal,null);assert.notEqual(h.ST.state.project,owner);
      assert.equal(h.ST.state.project.name,command==='undo'?'Unsaved piece':'Edited with take preview');
      const coverage=h.element('takes-coverage').textContent;
      if(finishPreview){finishPreview({});await tick();assert.equal(h.element('takes-coverage').textContent,coverage,'late preview completion must not revive the closed manager');}
      assert.equal(h.calls.filter(x=>x.command==='sample.stopPreview').length,1);
    }
    console.log('PASS actual take-manager preview stops before Undo/Redo project replacement through app and native menu, including late preview completion');
    for(const command of ['undo','redo'])for(const accepted of [true,false]){
      const h=takesHarness(command),owner=h.ST.state.project;let finishFlush,pending=true,closed=0;
      const flushed=new Promise(resolve=>finishFlush=resolve);
      h.window.STNKSControls={get hasPendingEdits(){return pending;},event(){},async flushEdits(){await flushed;pending=false;return accepted;}};
      h.element('takes-preview').click();await tick();
      const originalClose=h.window.STClipTakes.closed;h.window.STClipTakes.closed=()=>{assert.equal(h.ST.state.project,owner);closed++;originalClose();};
      h.window.nativeEvent({type:'command',payload:{name:command}});await tick();
      assert.equal(h.ST.state.project,owner);assert.equal(h.ST.state.modal,'clip-takes');assert.equal(closed,0);assert.equal(h.calls.filter(x=>x.command==='sample.stopPreview').length,0);
      finishFlush();await tick();
      assert.equal(closed,accepted?1:0);assert.equal(h.ST.state.modal,accepted?null:'clip-takes');assert.equal(h.calls.filter(x=>x.command==='sample.stopPreview').length,accepted?1:0);
      if(accepted)assert.notEqual(h.ST.state.project,owner);else assert.equal(h.ST.state.project,owner);
    }
    console.log('PASS take-manager preview closes only after a successful pending NKS flush accepts Undo/Redo');
    for(const blocker of ['empty','export','routing']){
      const h=takesHarness('undo'),owner=h.ST.state.project;h.element('takes-preview').click();await tick();
      if(blocker==='empty')h.ST.state.undo=[];
      if(blocker==='export')h.window.STExport={isBusy:true};
      if(blocker==='routing')h.window.STRouting={isApplying:true};
      await h.ST.perform('undo');assert.equal(h.ST.state.project,owner);assert.equal(h.ST.state.modal,'clip-takes');
      assert.equal(h.calls.filter(x=>x.command==='sample.stopPreview').length,0);
    }
    console.log('PASS unavailable or blocked Undo preserves the live take manager because no project replacement occurs');
  }
  {
    for(const outcome of ['stop','toggle','success','error']){
      const h=harness();let finish,reject,pending=true;
      const gate=new Promise((resolve,no)=>{finish=resolve;reject=no;});
      h.window.STNKSControls={get hasPendingEdits(){return pending;},async flushEdits(){await gate;pending=false;return true;}};
      const playing=h.ST.perform('play'),button=h.element('#play');
      assert.equal(button.textContent,'…');assert.equal(button.attributes['aria-busy'],'true');assert.equal(button.attributes['aria-label'],'Cancel preparing playback');assert.match(button.title,/Cancel preparing/);
      assert.match(h.element('#status-message').textContent,/Preparing playback/);await tick();
      if(outcome==='stop'||outcome==='toggle'){
        let stopped;const stopGate=new Promise(resolve=>stopped=resolve);h.handlers.set('transport.stop',()=>stopGate);
        const stopping=h.ST.perform(outcome==='stop'?'stop':'play');
        assert.equal(button.attributes['aria-busy'],'false');assert.equal(button.textContent,'▶');assert.equal(h.element('#status-message').textContent,'Ready to make something');
        stopped({});await stopping;finish();assert.equal(await playing,false);
      }else if(outcome==='success'){finish();assert.equal(await playing,true);assert.equal(button.textContent,'Ⅱ');assert.equal(button.attributes['aria-label'],'Pause playback');}
      else{reject(Error('Playback preparation failed'));assert.equal(await playing,false);assert.equal(button.textContent,'▶');assert.match(h.element('#toast-region').innerHTML,/Playback preparation failed/);}
      assert.equal(button.attributes['aria-busy'],'false');assert.doesNotMatch(button.title,/preparing/);assert.doesNotMatch(h.element('#status-message').textContent,/Preparing playback/);
    }
    console.log('PASS preparing playback feedback is immediate, cancellable, and clears on Stop, repeated Play, success or failure');
  }
  {
    {
      const h=harness();let finish,pending=true;const gate=new Promise(resolve=>finish=resolve);
      h.ST.state.recording=true;
      h.window.STNKSControls={get hasPendingEdits(){return pending;},async flushEdits(){await gate;pending=false;return true;}};
      const playing=h.ST.perform('play');assert.equal(h.element('#play').attributes['aria-busy'],'true');
      assert.match(h.element('#status-message').textContent,/Step recording armed/);
      await h.ST.perform('stop');finish();await playing;
      assert.match(h.element('#status-message').textContent,/Step recording armed/);
    }
    console.log('PASS pending playback keeps the recording status in the status line');
  }
  {
    for(const failure of ['mapped','native']){
      const h=harness();
      if(failure==='mapped')h.window.STNKSControls={hasPendingEdits:true,flushEdits:async()=>false};
      else h.handlers.set('engine.project',()=>{throw Error('Audio graph rejected');});
      assert.equal(await h.ST.perform('play'),false);assert.equal(h.ST.state.playing,false);
      assert(!h.calls.some(x=>x.command==='transport.seek'||x.command==='transport.play'));
      assert.match(h.element('#toast-region').innerHTML,/captur|rejected/);
      h.ST.state.playing=true;await h.ST.perform('play');assert.equal(h.ST.state.playing,false);assert(h.calls.some(x=>x.command==='transport.stop'));
    }
    console.log('PASS strict playback preparation failures never seek or play and cannot block Stop');
  }
  {
    for(const phase of ['mapped','seek'])for(const cancel of ['stop','toggle','project']){
      const h=harness();let finish,pending=true;
      const gate=new Promise(resolve=>finish=()=>{pending=false;resolve({});});
      if(phase==='mapped')h.window.STNKSControls={get hasPendingEdits(){return pending;},flushEdits:()=>gate};
      else h.handlers.set('transport.seek',()=>gate);
      const playing=h.ST.perform('play');await tick();
      if(cancel==='project')h.ST.state.project=h.ST.normalize({name:'Replacement'});
      else await h.ST.perform(cancel==='stop'?'stop':'play');
      finish();await playing;assert(!h.calls.some(x=>x.command==='transport.play'),phase+' '+cancel);assert.equal(h.ST.state.playing,false);
    }
    console.log('PASS Stop, repeated Play and project replacement cancel delayed playback starts before and after seek');
  }
  {
    const h=harness();let finish,pending=true;const gate=new Promise(resolve=>finish=resolve);
    h.window.STNKSControls={get hasPendingEdits(){return pending;},async flushEdits(){await gate;h.ST.state.project.audioPlugins=[{owner:'master',kind:'effect',slot:0,state:'settled mapped state'}];pending=false;return true;}};
    h.ST.state.view='song';h.ST.perform('modeSong');h.ST.state.row=13;h.ST.state.sequence=1;
    const playing=h.ST.perform('play');await tick();assert(!h.calls.some(x=>x.command==='engine.project'));finish();assert.equal(await playing,true);
    const synced=h.calls.find(x=>x.command==='engine.project');assert.equal(synced.payload.project.audioPlugins[0].state,'settled mapped state');
    const start=h.calls.find(x=>x.command==='transport.play');assert.equal(start.payload.mode,'song');assert.equal(start.payload.row,0);assert.equal(start.payload.sequence,1);
    console.log('PASS Play serializes settled mapped state and uses the same song start position for seek and play');
  }
  {
    function assignment(){
      const h=harness(),p=h.ST.state.project;p.instruments[1].type='au';p.tracks[0].instrument=0;p.tracks[1].instrument=1;
      p.audioPlugins=[{owner:'track:t0',kind:'instrument',slot:0,component:'old',state:'old target'},{owner:'track:t1',kind:'instrument',slot:0,component:'fixture',state:'old source'}];
      p.instruments[1].plugin={...p.audioPlugins[1]};h.ST.state.instrument=1;h.ST.state.track=0;h.ST.setView('instrument');
      // Assignment captures the retiring target before loading its replacement.
      let loaded=false;
      h.handlers.set('plugin.state',payload=>payload.owner==='track:t1'?{...p.audioPlugins[1]}:loaded?{owner:'track:t0',kind:'instrument',slot:0,component:'fixture',state:'new target'}:{...p.audioPlugins[0]});
      h.handlers.set('plugin.load',payload=>{loaded=true;return {...payload};});return h;
    }
    for(const failure of ['mapped','native']){
      const h=assignment(),before=JSON.stringify(h.ST.state.project);
      if(failure==='mapped')h.window.STNKSControls={hasPendingEdits:true,flushEdits:async()=>false};else h.handlers.set('engine.project',()=>{throw Error('Assignment graph failed');});
      await h.element('#assign-instrument').click();assert.equal(JSON.stringify(h.ST.state.project),before);assert(!h.calls.some(x=>x.command==='plugin.load'||x.command==='plugin.remove'||x.command==='plugin.state'));
    }
    const h=assignment();let finish,pending=true;const gate=new Promise(resolve=>finish=resolve);
    h.window.STNKSControls={get hasPendingEdits(){return pending;},async flushEdits(){await gate;h.ST.state.project.audioPlugins[1].state='settled source';pending=false;return true;}};
    const assigning=h.element('#assign-instrument').click();await tick();assert(!h.calls.some(x=>x.command==='plugin.load'||x.command==='plugin.state'));finish();await assigning;
    assert.equal(h.calls.find(x=>x.command==='plugin.load').payload.state,'settled source');assert.equal(h.ST.state.project.tracks[0].instrument,1);
    console.log('PASS instrument assignment settles pending mapped controls and rejects failed preparation before native changes');
    for(const operation of ['load','remove']){
      const s=assignment();if(operation==='remove')s.ST.state.project.instruments[1].type='synth';
      let finishNative;const gateNative=new Promise(resolve=>finishNative=resolve);s.handlers.set('plugin.'+operation,()=>gateNative);
      const assigning=s.element('#assign-instrument').click();await new Promise(resolve=>setImmediate(resolve));assert(s.calls.some(x=>x.command==='plugin.'+operation));
      const replacement=s.ST.normalize({name:'Different owner',tracks:[{id:0,name:'Track 01',routingID:'t0',instrument:0}],audioPlugins:[{owner:'track:t0',kind:'instrument',slot:0,component:'keep',state:'replacement state'}]});s.ST.state.project=replacement;
      finishNative({component:'fixture'});await assigning;
      const restored=s.calls.filter(x=>x.command==='engine.project').at(-1);assert.equal(restored.payload.project.name,'Different owner');assert.equal(restored.payload.replace,true);
      assert.equal(replacement.audioPlugins[0].state,'replacement state');assert.equal(replacement.audioPlugins[0].component,'keep');
    }
    console.log('PASS late plug-in assignment and removal restore the current owner without filtering its retained descriptors');
  }
  {
    // The boot placeholder never reaches the recovery copy while the host still restores the startup session.
    const h=harness();h.ST.state.native=true;let release;h.handlers.set('ready',()=>new Promise(resolve=>{release=resolve;}));
    const booting=h.ST.boot();await tick();
    assert.equal(h.ST.startupPending,true);assert.equal(h.ST.state.project.name,'Untitled session');
    assert.notEqual(h.ST.mutate('placeholder edit',p=>{p.name='Typed during launch';}),false);h.advance(5000);await tick();
    assert(!h.calls.some(x=>x.command==='project.autosave'),'no autosave of the placeholder');
    assert.equal(h.window.ST.startupPending,true,'the quit checkpoint reads startupPending and keeps the host session');
    release({project:{name:'Recovered unsaved work'},dirty:true,roots:[],plugins:[],audio:{},midi:{}});await booting;
    assert.equal(h.ST.startupPending,false);assert.equal(h.ST.state.project.name,'Recovered unsaved work');
    h.ST.mutate('real edit',p=>{p.bpm=121;});h.advance(5000);await tick();
    const autosaved=h.calls.filter(x=>x.command==='project.autosave');assert.equal(autosaved.length,1);assert.equal(autosaved[0].payload.project.name,'Recovered unsaved work');
    console.log('PASS the boot placeholder is never autosaved over the session the host is still restoring');
  }
  {
    // A startup session whose audio failed still opens, and its audio is retried as a full replacement.
    const h=harness();h.ST.state.native=true;h.handlers.set('ready',()=>({project:{name:'Unsaved break'},dirty:true,audioError:'Audio engine refused routing change',roots:[],plugins:[]}));
    await h.ST.boot();assert.equal(h.ST.state.project.name,'Unsaved break');assert.equal(h.ST.startupPending,false);
    const sent=h.calls.filter(x=>x.command==='engine.project').at(-1);assert.equal(sent.payload.project.name,'Unsaved break');assert.equal(sent.payload.replace,true);
    console.log('PASS a startup session whose audio failed still reaches the window and is republished as a replacement');
  }
  {
    const h=harness();h.handlers.set('ready',()=>({project:{name:'Needs repair'},roots:[],plugins:[]}));h.handlers.set('engine.project',()=>{throw Error('Startup routing rejected');});
    await h.ST.boot();assert.match(h.element('#toast-region').innerHTML,/Audio could not be prepared.*Startup routing rejected/);assert(!h.calls.some(x=>x.command==='transport.play'));
    h.handlers.set('engine.project',()=>({}));h.window.nativeEvent({type:'project.openRequested',payload:{path:'/tmp/repaired.supertracker'}});await new Promise(resolve=>setImmediate(resolve));
    assert(h.calls.some(x=>x.command==='project.open'&&x.payload.path==='/tmp/repaired.supertracker'));
    console.log('PASS failed startup preparation stays visible and leaves guarded project opening available for recovery');
  }
  {
    for(const deliberate of [false,true]){
      const h=harness(),dialog=h.element('.modal'),close=h.element('#modal-close'),field=h.element('#routing-search'),button=h.element('#routing-choice');
      for(const [node,tag,body] of [[close,'BUTTON',false],[field,'INPUT',true],[button,'BUTTON',true]]){
        node.tagName=tag;node.tabIndex=0;node.offsetParent={};node.hidden=false;node.focus=()=>h.document.activeElement=node;node.closest=selector=>selector==='.modal-body'&&body?{}:null;
      }
      const nodes=[close,field,button];dialog.querySelectorAll=()=>nodes;dialog.contains=node=>nodes.includes(node);
      h.ST.openModal('Route output','Body');if(deliberate)button.focus();h.advance(0);
      assert.equal(h.document.activeElement,deliberate?button:field);
    }
    console.log('PASS delayed modal initialization preserves deliberate focus and otherwise prefers its body field');
  }
  {
    const h=harness(),old=h.element('.modal'),field=h.element('#old-field');let focused=0;
    Object.assign(field,{tagName:'INPUT',tabIndex:0,offsetParent:{},focus(){focused++;},closest(){return {};}});old.querySelectorAll=()=>[field];old.contains=()=>false;
    h.ST.openModal('First','Body');h.ST.closeModal();focused=0;h.advance(0);assert.equal(focused,0);
    h.ST.openModal('Second','Body');const query=h.document.querySelector;h.document.querySelector=selector=>selector==='.modal'?{querySelectorAll(){return[];}}:query(selector);h.advance(0);assert.equal(focused,0);
    console.log('PASS delayed modal focus cannot reach a closed or replaced dialog');
  }
  {
    const h=harness();let epoch=0,finishCapture;
    h.ST.state.engineReady=true;h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'instrument',slot:0,state:'edited'}];
    h.handlers.set('engine.project',()=>{epoch++;return {};});
    h.handlers.set('engine.snapshot',()=>{const startEpoch=epoch;return new Promise((resolve,reject)=>{finishCapture=()=>epoch===startEpoch?resolve({audioPlugins:h.ST.state.project.audioPlugins}):reject(new Error('The project or audio graph changed while plug-in edits were being saved.'));});});
    h.ST.mutate('note before Save',p=>p.patterns[0].steps[0][0].note=72);
    const save=h.ST.projectAction('save');await tick();h.advance(1000);await tick();finishCapture();
    assert.equal(await save,true,'a queued note sync must finish before asynchronous plug-in capture');
    assert.deepEqual(h.calls.filter(x=>['engine.project','engine.snapshot','project.save'].includes(x.command)).map(x=>x.command),['engine.project','engine.snapshot','project.save']);
    assert.equal(h.calls.find(x=>x.command==='project.save').payload.project.patterns[0].steps[0][0].note,72);
    console.log('PASS Save drains an actual queued note sync before asynchronous plug-in capture');
  }
  {
    const h=harness();let finishGraph,moving=true;
    const graph=new Promise(resolve=>finishGraph=()=>{moving=false;resolve();});
    h.window.STStudio={get isMovingEffect(){return moving;},waitForEffectMove:()=>graph};
    h.ST.state.engineReady=true;h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'effect',slot:0,state:'current'}];
    h.handlers.set('engine.snapshot',()=>({audioPlugins:h.ST.state.project.audioPlugins}));
    h.ST.mutate('queued note',p=>p.patterns[0].steps[0][0].note=73);h.advance(101);await tick();
    const save=h.ST.projectAction('save');await tick();
    assert.equal(h.calls.some(x=>x.command==='engine.project'||x.command==='engine.snapshot'),false);
    finishGraph();assert.equal(await save,true);h.advance(1000);await tick();
    assert.deepEqual(h.calls.filter(x=>['engine.project','engine.snapshot','project.save'].includes(x.command)).map(x=>x.command),['engine.project','engine.snapshot','project.save']);
    assert.equal(h.calls.find(x=>x.command==='project.save').payload.project.patterns[0].steps[0][0].note,73);
    console.log('PASS Save waits for an already-running graph-gated sync without posting it twice');
  }
  {
    const h=harness();let finishSync,first=true;
    h.ST.state.engineReady=true;h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'effect',slot:0,state:'current'}];
    h.handlers.set('engine.project',()=>first?(first=false,new Promise(resolve=>finishSync=()=>resolve({}))):{});
    h.handlers.set('engine.snapshot',()=>({audioPlugins:h.ST.state.project.audioPlugins}));
    const syncing=h.ST.syncEngine(true);await tick();
    const save=h.ST.projectAction('save');await tick();
    assert.equal(h.calls.some(x=>x.command==='engine.snapshot'),false);
    h.ST.mutate('note while engine sync finishes',p=>p.patterns[0].steps[0][0].note=74);
    finishSync();await syncing;assert.equal(await save,true);
    assert.deepEqual(h.calls.filter(x=>['engine.project','engine.snapshot','project.save'].includes(x.command)).map(x=>x.command),['engine.project','engine.project','engine.snapshot','project.save']);
    assert.equal(h.calls.find(x=>x.command==='project.save').payload.project.patterns[0].steps[0][0].note,74);
    console.log('PASS Save awaits an in-flight sync and also drains notes queued during that wait');
  }
  {
    for(const mode of ['save','export-preparation']){
      const h=harness();h.ST.state.engineReady=true;
      h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'instrument',slot:0,state:'original patch'}];
      h.handlers.set('engine.snapshot',()=>({audioPlugins:h.ST.state.project.audioPlugins}));
      h.ST.mutate('change patch',p=>p.audioPlugins[0].state='new patch');h.ST.perform('undo');
      h.ST.syncEngine(); // An ordinary refresh must not erase Undo's replacement flag.
      if(mode==='export-preparation'){h.window.STExport={isBusy:true};h.advance(1000);await tick();assert.equal(h.calls.some(x=>x.command==='engine.project'),false);}
      assert.equal(await (mode==='save'?h.ST.projectAction('save'):h.ST.captureNativeEdits()),true);
      const sends=h.calls.filter(x=>x.command==='engine.project');assert.equal(sends.length,1);assert.equal(sends[0].payload.replace,true);
      assert.equal(sends[0].payload.project.audioPlugins[0].state,'original patch');
    }
    console.log('PASS Undo replacement state survives debounce coalescing and locked export preparation');
  }
  {
    const h=harness();let busy=true;
    h.window.STExport={get isBusy(){return busy;}};
    h.ST.state.project.patterns[0].steps[0][0].note=75;h.ST.syncEngine();h.advance(1000);await tick();
    assert.equal(h.calls.some(x=>x.command==='engine.project'),false);
    busy=false;h.advance(101);await tick();
    assert.equal(h.calls.filter(x=>x.command==='engine.project').length,1);
    assert.equal(h.calls.find(x=>x.command==='engine.project').payload.project.patterns[0].steps[0][0].note,75);
    console.log('PASS a destination cancellation leaves queued notes eligible for their normal engine sync');
  }
  {
    const h=harness();let failFirst,finishSecond,number=0;
    h.ST.state.engineReady=true;h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'effect',slot:0,state:'keep patch'}];
    h.handlers.set('engine.project',()=>++number===1?new Promise((resolve,reject)=>failFirst=()=>reject(new Error('First update failed'))):new Promise(resolve=>finishSecond=()=>resolve({})));
    const first=h.ST.syncEngine(true);await tick();const second=h.ST.syncEngine(true);await tick();
    let settled=false;const save=h.ST.projectAction('save').then(value=>{settled=true;return value;});await tick();
    failFirst();await first;await tick();assert.equal(settled,false);
    assert.equal(h.calls.some(x=>x.command==='engine.snapshot'||x.command==='routing.status'),false);
    finishSecond();await second;assert.equal(await save,false);
    assert.equal(h.calls.some(x=>x.command==='project.save'),false);
    console.log('PASS capture settles every active engine update even when an earlier update rejects');
  }
  {
    const h=harness();h.ST.state.engineReady=true;
    h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'instrument',slot:0,state:'keep patch'}];
    h.handlers.set('engine.project',()=>{throw new Error('Audio graph update failed');});
    h.ST.mutate('note before failed sync',p=>p.patterns[0].steps[0][0].note=76);
    assert.equal(await h.ST.projectAction('save'),false);
    assert.match(h.element('#toast-region').innerHTML,/Audio graph update failed/);
    assert.equal(h.ST.state.project.patterns[0].steps[0][0].note,76);assert.equal(h.ST.state.dirty,true);
    assert.equal(h.calls.some(x=>x.command==='engine.snapshot'||x.command==='project.save'),false);
    console.log('PASS a failed queued engine update is reported and retains unsaved notes');
  }
  {
    const h=harness();let finishGraph,moving=true;
    const graph=new Promise(resolve=>finishGraph=()=>{moving=false;resolve();});
    h.window.STStudio={get isMovingEffect(){return moving;},waitForEffectMove:()=>graph};
    h.window.STStudio.event=()=>{};
    h.ST.syncEngine(false,true);h.advance(101);await tick();
    const replacement=h.ST.normalize({name:'New session'});h.ST.acceptProject(replacement);
    finishGraph();h.advance(101);await tick();
    const sends=h.calls.filter(x=>x.command==='engine.project');assert.equal(sends.length,1);
    assert.equal(sends[0].payload.project.name,'New session');assert.equal(sends[0].payload.replace,undefined);
    console.log('PASS an obsolete graph-waiting sync cannot carry Undo state into a replacement session');
  }
  {
    const h=harness();let complete;
    h.ST.mutate('edit before save',p=>p.name='Waiting for Save');
    h.handlers.set('project.save',()=>new Promise(resolve=>complete=()=>resolve({saved:true,project:h.ST.state.project})));
    const save=h.ST.projectAction('save');await tick();h.advance(2000);await tick();
    assert.equal(h.calls.some(x=>x.command==='project.autosave'),false);
    complete();assert.equal(await save,true);h.advance(1000);await tick();
    assert.equal(h.calls.filter(x=>x.command==='project.autosave').length,1);
    console.log('PASS scheduled recovery defers during explicit Save and resumes afterward');
  }
  {
    const h=harness();let captures=0;
    h.handlers.set('project.autosave',()=>++captures===1?{saved:false,deferred:true}:{saved:true});
    h.ST.mutate('initial edit',p=>p.name='Initial edit');h.advance(2000);await tick();
    assert.equal(captures,1);h.ST.state.project.name='Newest edit';h.advance(1000);await tick();
    assert.equal(captures,2);assert.equal(h.calls.filter(x=>x.command==='project.autosave').at(-1).payload.project.name,'Newest edit');
    console.log('PASS a deferred native recovery request retries with the current project');
  }
  {
    const h=harness(),close=h.element('modal-close'),search=h.element('routing-search'),option=h.element('routing-choice-0');
    for(const el of [close,search,option]){el.offsetParent={};el.hidden=false;el.focus=()=>h.document.activeElement=el;}
    close.tabIndex=0;search.tabIndex=0;option.tabIndex=-1;
    h.element('.modal').querySelectorAll=()=>[close,search,option];h.ST.state.modal='routing-selector';
    search.focus();let prevented=false;
    h.ST.keyboard({key:'Tab',target:search,preventDefault(){prevented=true;}});
    assert.equal(prevented,true);assert.equal(h.document.activeElement,close);
    prevented=false;h.ST.keyboard({key:'Tab',shiftKey:true,target:close,preventDefault(){prevented=true;}});
    assert.equal(prevented,true);assert.equal(h.document.activeElement,search);
    console.log('PASS modal Tab wrap excludes arrow-only routing options from its native tab order');
  }
  {
    for(const phase of ['timer','graph-wait']){
      const h=harness();let complete,moving=phase==='graph-wait';
      const done=new Promise(resolve=>complete=()=>{moving=false;resolve();});
      h.window.STStudio={get isMovingEffect(){return moving;},waitForEffectMove:()=>done};
      h.ST.syncEngine();if(phase==='graph-wait'){h.advance(101);await tick();}
      h.ST.cancelPendingEngineSync();complete();h.advance(101);await tick();
      assert.equal(h.calls.some(call=>call.command==='engine.project'),false);
      await h.ST.syncEngine(true);assert.equal(h.calls.filter(call=>call.command==='engine.project').length,1);
    }
    console.log('PASS strict export preparation can invalidate both queued and graph-waiting engine sync');
  }
  {
    for(const action of ['save','open']){
      const h=harness();let complete,shown=0;
      h.ST.state.dirty=false;
      h.window.STExport={show(){shown++;return true;},isBusy:false};
      h.handlers.set('project.'+action,()=>new Promise(resolve=>complete=()=>resolve(action==='save'?{saved:true,project:h.ST.state.project}:{project:{name:'Opened piece'}})));
      const pending=h.ST.projectAction(action);await tick();
      assert.equal(h.ST.isChangingSession,true);
      assert.equal(await h.ST.perform('export'),false);assert.equal(shown,0);
      let prevented=false;h.ST.keyboard({key:'E',metaKey:true,shiftKey:true,target:h.element('body'),preventDefault(){prevented=true;}});
      assert.equal(prevented,true);assert.equal(shown,0);
      complete();assert.equal(await pending,true);assert.equal(h.ST.isChangingSession,false);
      assert.equal(await h.ST.perform('export'),true);assert.equal(shown,1);
    }
    console.log('PASS pending Save and Open defer export until their exact native replies complete');
  }
  {
    for(const operation of ['export','recording']){
      const h=harness();let complete,busy=false;
      h.ST.state.engineReady=true;h.ST.state.dirty=false;h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'effect',slot:0}];
      h.window.STExport={get isBusy(){return operation==='export'&&busy;}};
      h.window.STRecording={get isBusy(){return operation==='recording'&&busy;}};
      h.handlers.set('engine.snapshot',()=>new Promise(resolve=>complete=()=>resolve({audioPlugins:h.ST.state.project.audioPlugins})));
      const pending=h.ST.projectAction('new');await tick();busy=true;complete();
      assert.equal(await pending,false);assert.equal(h.calls.some(call=>call.command==='project.new'),false);
      assert.equal(h.ST.state.project.name,'Unsaved piece');
    }
    console.log('PASS session replacement rechecks export and recording after asynchronous state capture');
  }
  {
    const h=harness();let busy=true,cancelled=0;
    h.window.STExport={get isBusy(){return busy;},cancel(){cancelled++;}};
    h.ST.state.modal='export';const original=h.ST.state.project.name;
    h.ST.perform('new');assert.equal(await h.ST.projectAction('open'),false);assert.equal(await h.ST.projectAction('save'),false);
    h.ST.mutate('edit during export',p=>p.name='Incorrect');assert.equal(h.ST.state.project.name,original);
    h.ST.closeModal();assert.equal(h.ST.state.modal,'export');
    h.ST.openModal('Another dialog','Example');assert.equal(h.ST.state.modal,'export');
    let prevented=false;h.ST.keyboard({key:'Escape',preventDefault(){prevented=true;}});
    assert.equal(cancelled,1);assert.equal(prevented,true);assert.equal(h.calls.some(call=>call.command.startsWith('project.')),false);
    busy=false;h.ST.closeModal();assert.equal(h.ST.state.modal,null);
    console.log('PASS active export protects the session and dialog while Escape requests cancellation');
  }
  {
    const h=harness();let complete;
    h.ST.state.engineReady=true;h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'effect',slot:0,state:'old'}];
    h.handlers.set('engine.snapshot',()=>new Promise(resolve=>complete=()=>resolve({audioPlugins:[{owner:'track:t0',kind:'effect',slot:0,state:'captured'}]})));
    const save=h.ST.projectAction('save');await tick();
    h.ST.state.project.audioPlugins=[];complete();
    assert.equal(await save,false);assert.equal(h.ST.state.project.audioPlugins.length,0);
    assert.equal(h.calls.some(call=>call.command==='project.save'),false);
    console.log('PASS a concurrent plug-in removal cannot be overwritten by an older state capture');
  }
  {
    const h=harness();let complete,moving=true;
    const done=new Promise(resolve=>complete=()=>{moving=false;resolve();});
    h.window.STStudio={get isMovingEffect(){return moving;},waitForEffectMove:()=>done};
    const pending=h.ST.syncEngine(true);h.ST.mutate('parallel pattern edit',p=>p.name='Edit during move');
    h.advance(2000);await tick();
    assert.equal(h.calls.filter(x=>['engine.project','project.autosave'].includes(x.command)).length,0);
    h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'effect',slot:1,instanceID:'moved'}];complete();await pending;await tick();
    const sends=h.calls.filter(x=>['engine.project','project.autosave'].includes(x.command));
    assert.ok(sends.some(x=>x.command==='project.autosave'));assert.ok(sends.some(x=>x.command==='engine.project'));
    for(const send of sends){assert.equal(send.payload.project.name,'Edit during move');assert.equal(send.payload.project.audioPlugins[0].slot,1);}
    console.log('PASS engine sync and autosave wait for complete insert remapping and retain parallel edits');
  }
  {
    const h=harness();let complete,moving=true;
    const done=new Promise(resolve=>complete=()=>{moving=false;resolve();});
    h.window.STStudio={get isMovingEffect(){return moving;},waitForEffectMove:()=>done};
    h.ST.state.engineReady=true;h.ST.state.project.audioPlugins=[{owner:'track:t0',kind:'effect',slot:0}];
    h.handlers.set('engine.snapshot',()=>({audioPlugins:[{owner:'track:t0',kind:'effect',slot:1}]}));
    const save=h.ST.projectAction('save');await tick();
    assert.equal(h.calls.some(x=>x.command==='engine.snapshot'||x.command==='project.save'),false);
    complete();assert.equal(await save,true);
    assert.equal(h.calls.find(x=>x.command==='project.save').payload.project.audioPlugins[0].slot,1);
    console.log('PASS project save waits for insert movement before capturing native state');
  }
  {
    const h=harness(),origin=h.element('#browser-inspect');let focusCount=0;origin.focus=()=>focusCount++;
    h.document.activeElement=origin;h.ST.state.view='browser';
    h.ST.openModal('First dialog','Example');h.document.activeElement=h.element('#modal-close');
    h.ST.openModal('Replacement dialog','Example');h.ST.closeModal();
    assert.equal(focusCount,1);assert.equal(h.ST.state.modal,null);
    h.document.activeElement=origin;h.ST.openModal('Removed control','Example');origin.isConnected=false;origin.id='';
    let searchFocused=false;h.element('#browser-search').focus=()=>searchFocused=true;h.ST.closeModal();
    assert.equal(searchFocused,true);
    console.log('PASS dialogs return focus to their invoker or the current workspace after replacement/removal');
  }
  {
    const h=harness();let closed=0,busy=true,recording=true;
    h.window.STRecording={get isBusy(){return busy;},get isRecording(){return recording;},closed(){closed++;},event(){},show(){h.ST.state.modal='recording';}};
    h.ST.showRecording();h.ST.closeModal();assert.equal(h.ST.state.modal,'recording');assert.equal(closed,0);
    assert.equal(await h.ST.perform('new'),undefined);assert.equal(h.calls.filter(x=>x.command==='project.new').length,0);
    h.ST.openModal('Other dialog','Must not replace recording');assert.equal(h.ST.state.modal,'recording');
    busy=false;recording=false;h.ST.openModal('Other dialog','Idle recording can close');assert.equal(closed,1);assert.equal(h.ST.state.modal,'dialog');
    h.ST.showRecording();h.ST.closeModal();assert.equal(closed,2);assert.equal(h.ST.state.modal,null);
    console.log('PASS native recording busy state blocks close/session changes and cleans up idle modal replacements');
  }
  {
    const h=harness(),instrument=h.ST.state.project.instruments[0];
    Object.assign(instrument,{samplePath:'/old.wav',slicePoints:[0,.3,.7],slices:[0,.5],zones:[{samplePath:'/zone.wav'}]});
    h.handlers.set('sample.load',()=>({path:'/new.wav',name:'Fresh sample'}));
    await h.ST.loadSample('/new.wav',0);
    assert.equal(instrument.samplePath,'/new.wav');assert.equal(instrument.slicePoints.length,0);assert.equal(instrument.zones.length,0);assert.equal(instrument.slices.length,0);
    h.ST.perform('undo');assert.equal(h.ST.state.project.instruments[0].samplePath,'/old.wav');assert.equal(h.ST.state.project.instruments[0].zones[0].samplePath,'/zone.wav');
    console.log('PASS replacing a sample removes old slice/zone routing and Undo restores it');
  }
  {
    const h=harness(),original=h.ST.state.project,pending=h.ST.perform('new');await tick();
    assert.equal(h.ST.state.modal,'unsaved');assert.ok(h.element('#overlay').innerHTML.includes('Discard'));
    h.element('#unsaved-cancel').click();assert.equal(await pending,false);assert.equal(h.ST.state.project,original);assert.equal(h.calls.filter(x=>x.command==='project.new').length,0);
    console.log('PASS Cancel preserves unsaved session');
  }
  {
    const h=harness(),pending=h.ST.perform('new');await tick();h.element('#unsaved-discard').click();await pending;
    assert.equal(h.ST.state.project.name,'Untitled session');assert.equal(h.calls.filter(x=>x.command==='project.save').length,0);
    console.log('PASS Discard starts requested new session without saving');
  }
  {
    const h=harness();let completeSave;h.handlers.set('project.save',payload=>new Promise(resolve=>completeSave=()=>resolve({saved:true,project:payload.project})));
    const pending=h.ST.projectAction('open','/tmp/finder.supertracker');await tick();const save=h.element('#unsaved-save').click();await tick();
    assert.equal(h.calls.filter(x=>x.command==='project.open').length,0);assert.equal(h.ST.state.project.name,'Unsaved piece');
    completeSave();await save;await pending;
    assert.equal(h.calls.find(x=>x.command==='project.open').payload.path,'/tmp/finder.supertracker');assert.equal(h.ST.state.project.name,'Opened piece');
    console.log('PASS Save is awaited before opening explicit Finder path');
  }
  {
    const h=harness();h.handlers.set('project.save',()=>({cancelled:true}));const original=h.ST.state.project,pending=h.ST.midiFileAction(true);await tick();await h.element('#unsaved-save').click();await pending;
    assert.equal(h.ST.state.project,original);assert.equal(h.ST.state.dirty,true);assert.equal(h.calls.filter(x=>x.command==='project.importMIDI').length,0);
    console.log('PASS Canceling native Save also cancels MIDI replacement');
  }
  {
    const h=harness();h.handlers.set('project.save',()=>{throw Error('Disk is read-only');});const pending=h.ST.perform('demo');await tick();await h.element('#unsaved-save').click();await pending;
    assert.equal(h.ST.state.project.name,'Unsaved piece');assert.equal(h.calls.filter(x=>x.command==='project.new').length,0);
    console.log('PASS Failed save never discards the current session');
  }
  {
    const h=harness(),pending=h.ST.perform('demo');await tick();h.ST.closeModal();assert.equal(await pending,false);assert.equal(h.ST.state.project.name,'Unsaved piece');
    const second=h.ST.perform('demo');await tick();h.element('#unsaved-discard').click();await second;
    assert.equal(h.ST.state.project.name,'Midnight transmission');assert.equal(h.calls.filter(x=>x.command==='project.new').length,1);
    console.log('PASS Closing prompt cancels; discarded demo resets native document identity');
  }
  {
    const h=harness();h.ST.state.dirty=false;h.ST.state.engineReady=true;h.ST.state.project.audioPlugins=[{owner:'master',kind:'effect',slot:0,state:'old'}];h.handlers.set('engine.snapshot',()=>({audioPlugins:[{owner:'master',kind:'effect',slot:0,state:'edited in native window'}]}));
    const pending=h.ST.perform('new');await tick();assert.equal(h.ST.state.modal,'unsaved');await h.element('#unsaved-save').click();await pending;
    assert.equal(h.calls.find(x=>x.command==='project.save').payload.project.audioPlugins[0].state,'edited in native window');
    console.log('PASS Native plug-in edits trigger protection and are included in Save');
  }
  {
    for(const malformed of [false,true]){
      const h=harness();h.ST.state.dirty=false;h.ST.state.engineReady=true;
      h.ST.state.project.audioPlugins=[{owner:'master',kind:'effect',slot:0,state:'unsecured native edits'}];const original=h.ST.state.project;
      h.handlers.set('engine.snapshot',()=>{if(malformed)return {};throw Error('State capture failed');});
      assert.equal(await h.ST.projectAction('save'),false);assert.equal(await h.ST.perform('new'),false);
      assert.equal(h.ST.state.project,original);assert.equal(h.ST.state.dirty,true);
      assert.equal(h.calls.filter(x=>x.command==='project.save'||x.command==='project.new').length,0);
    }
    console.log('PASS failed or malformed native state capture preserves the session and blocks unsafe save/replacement');
  }
  {
    const h=harness();h.ST.state.engineReady=true;h.ST.state.dirty=false;
    h.ST.state.project.audioPlugins=[{track:0,name:'Instrument',state:'previous valid state'}];
    const original=h.ST.state.project;
    h.handlers.set('engine.snapshot',()=>({audioPlugins:[{track:0,name:'Instrument',state:'fallback from previous save',stateError:'Render is still busy'}]}));
    assert.equal(await h.ST.projectAction('save'),false);assert.equal(await h.ST.perform('new'),false);
    assert.equal(h.ST.state.project,original);assert.equal(h.ST.state.project.audioPlugins[0].state,'previous valid state');
    assert.equal(h.ST.state.dirty,true);assert.equal(h.calls.some(x=>x.command==='project.save'||x.command==='project.new'),false);
    console.log('PASS a per-plug-in capture error cannot silently save fallback state or replace the session');
  }
  {
    const h=harness();h.handlers.set('plugin.state',()=>({state:'previous valid state',stateError:'Processor state unavailable'}));
    await assert.rejects(h.ST.request('plugin.state',{owner:'track:t0',kind:'instrument',slot:0}),/Processor state unavailable/);
    console.log('PASS plug-in state callers reject a native fallback carrying a capture error');
  }
  {
    const h=harness();h.ST.state.dirty=false;h.handlers.set('ready',()=>({project:{name:'Restored'},openPath:'/tmp/startup.supertracker',roots:[],plugins:[],audio:{},midi:{}}));await h.ST.boot();
    assert.equal(h.calls.find(x=>x.command==='project.open').payload.path,'/tmp/startup.supertracker');
    h.ST.state.dirty=true;h.window.nativeEvent({type:'project.openRequested',payload:{path:'/tmp/second.supertracker'}});for(let attempt=0;attempt<5&&h.ST.state.modal!=='unsaved';attempt++)await tick();assert.equal(h.ST.state.modal,'unsaved');h.element('#unsaved-discard').click();await tick();
    assert.equal(h.calls.filter(x=>x.command==='project.open').at(-1).payload.path,'/tmp/second.supertracker');
    console.log('PASS Both ready.openPath and subsequent Finder events use guarded opening');
  }
  {
    const h=harness();h.ST.state.dirty=false;h.handlers.set('ready',()=>({project:{name:'Recovered unsaved work'},dirty:true,roots:[],plugins:[],audio:{},midi:{}}));await h.ST.boot();
    assert.equal(h.ST.state.dirty,true);const original=h.ST.state.project,pending=h.ST.perform('new');await tick();
    assert.equal(h.ST.state.modal,'unsaved');h.element('#unsaved-cancel').click();await pending;assert.equal(h.ST.state.project,original);
    console.log('PASS Recovered unsaved session retains dirty state and replacement protection');
  }
  {
    const h=harness();h.ST.state.dirty=false;let finishOpen;h.handlers.set('project.open',()=>new Promise(resolve=>finishOpen=resolve));
    let complete=false;const pending=h.ST.projectAction('open').then(()=>complete=true);await tick();h.advance(60000);await tick();
    assert.equal(complete,false);assert.equal(h.ST.state.project.name,'Unsaved piece');
    finishOpen({project:{name:'Project chosen after a long dialog'}});await pending;
    assert.equal(h.ST.state.project.name,'Project chosen after a long dialog');
    console.log('PASS Native Open remains pending beyond15seconds and accepts the eventual project');
  }
  {
    const commands=['audio.export','audio.bounceTrack','routing.apply','project.save','project.saveAs','project.importMIDI','project.exportMIDI','dialog.openSample','dialog.export','project.media.chooseRoot','library.chooseRoot','audio.record','audio.configure','plugin.load','plugin.loadNKS'];
    for(const command of commands){const h=harness();let finish;h.handlers.set(command,()=>new Promise(resolve=>finish=resolve));let complete=false;const pending=h.ST.request(command).then(()=>complete=true);await tick();h.advance(300000);await tick();assert.equal(complete,false,command+' must await the user');finish({cancelled:true});await pending;}
    console.log('PASS Native file/permission panels and plug-in loads do not time out with a late native completion');
  }

}
run().catch(error=>{console.error(error);process.exitCode=1;});
