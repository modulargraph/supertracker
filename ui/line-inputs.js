/* Live interface channels into ordinary mixer tracks; explicit, acknowledged edits. */
(() => {
  'use strict';
  const app = () => window.ST, get = id => document.getElementById(id);
  const copy = value => JSON.parse(JSON.stringify(value));
  const esc = value => app().escapeHTML(String(value ?? ''));
  let session = null, opening = false;
  const binding = track => track.audioInput ? copy(track.audioInput) : null;
  // Monitoring is on unless switched off; only the off state is saved.
  const withMonitor = (value,monitor) => { const next={...value}; if(monitor)delete next.monitor; else next.monitor=false; return next; };
  // Host view of each saved route: live (maybe through an aggregate), or paused while its interface is absent.
  const routeStatus = (audio,index) => (audio?.lineInputs||[]).find(item=>item.track===index) || null;
  const routeNote = status => status?.state==='paused' ? String(status.status||'Interface not connected — returns paused') : status?.state==='live'&&status.via ? `Live through ${status.via}` : '';
  const current = s => session === s && app().state.project === s.project && s.project.tracks.length===s.tracks.length && s.tracks.every((track,i) => s.project.tracks[i] === track && JSON.stringify(track.audioInput ?? null) === JSON.stringify(s.original[i]));
  function message(s,text) { if (session === s && get('line-input-status')) get('line-input-status').textContent = text; }
  function controls(s) {
    for(const row of s.rows)for(const part of ['enabled','monitor','device','left','right','gain'])if(get(`line-input-${part}-${row.track}`))get(`line-input-${part}-${row.track}`).disabled=s.busy;
    for(const id of ['line-input-apply','line-input-close'])if(get(id))get(id).disabled=s.busy;
  }
  function read(s) {
    return s.rows.map(row => {
      const index=row.track, enabled=!!get(`line-input-enabled-${index}`).checked;
      const uid=get(`line-input-device-${index}`).value, device=s.devices.find(d=>d.uid===uid), monitor=!!get(`line-input-monitor-${index}`).checked;
      const left=Number(get(`line-input-left-${index}`).value), right=get(`line-input-right-${index}`).value;
      const channels=right==='mono'?[left]:[left,Number(right)], gain=Number(get(`line-input-gain-${index}`).value);
      const original=row.original, unchanged=original&&original.enabled&&enabled&&uid===original.deviceUID&&channels.join()===(original.channels||[]).join()&&gain===(original.gain??1)&&monitor===(original.monitor!==false);
      if(unchanged && uid!==s.selectedUID && s.kept.has(index))return {track:index,...copy(original)};
      if(enabled && uid!==s.selectedUID)throw new Error(`Select the current Studio interface for ${s.tracks[index].name || 'this track'}, or change the interface in Studio settings.`);
      if(enabled && (!device || !uid))throw new Error(`Choose an available interface for ${s.tracks[index].name || 'this track'}.`);
      if(enabled && (!channels.every(c=>Number.isInteger(c)&&c>=0&&c<Number(device.inputChannels)) || new Set(channels).size!==channels.length))throw new Error(`Choose valid, distinct input channels for ${s.tracks[index].name || 'this track'}.`);
      if(!String(get(`line-input-gain-${index}`).value).trim()||!Number.isFinite(gain)||gain<0||gain>2)throw new Error('Input gain must be between 0 and 2.');
      // Disabled unavailable bindings retain their exact saved map for reconnecting.
      if(!enabled && row.original && !row.original.enabled && uid===row.original.deviceUID && channels.join()===row.original.channels?.join() && gain===(row.original.gain??1) && monitor===(row.original.monitor!==false))return {track:index,...copy(row.original)};
      return withMonitor({track:index,...(row.original||{}),enabled,deviceUID:uid,channels,gain},monitor);
    }).filter(item=>s.original[item.track] || item.enabled);
  }
  async function restore(changed) {
    const state=app().state;
    if(state.project!==changed.project){
      if(app().syncEngineProject)await app().syncEngineProject(()=>copy(app().state.project),{replace:true,lineInputCommit:true});
      return;
    }
    await app().request('audio.inputs',{inputs:changed.rows.map(row=>({track:row.track,...(binding(state.project.tracks[row.track])||{enabled:false})}))},{lineInputCommit:true});
  }
  async function apply(s) {
    if(s.busy||!current(s)||app().isChangingSession)return;
    let inputs;try{inputs=read(s);}catch(error){message(s,error.message);return;}
    const changes=inputs.filter(({track,...value})=>JSON.stringify(value)!==JSON.stringify(s.original[track]) || value.enabled && !s.activeTracks.has(track) && !s.pausedTracks.has(track));
    if(!changes.length){app().closeModal();return;}
    s.busy=true;controls(s);message(s,'Applying input routing…');let attempted=false;
    try {
      if(await app().captureNativeEdits?.()===false)throw new Error('Could not preserve current instrument settings. Try again.');
      await app().waitForGraphEdit?.();
      await app().waitForEngineUpdate?.();
      if(!current(s)||app().isChangingSession)throw new Error('The project changed. Open line inputs again.');
      attempted=true;
      const result=await app().request('audio.inputs',{inputs:changes},{lineInputCommit:true});
      if(!result || typeof result!=='object' || !Array.isArray(result.inputs) || !Array.isArray(result.lineInputs))throw new Error('The audio host did not confirm input routing.');
      if(!current(s))throw new Error('The project changed while applying inputs.');
      let applied=false;
      const accepted=app().mutate('Configure line inputs',project=>{
        for(const {track,...value} of changes)project.tracks[track].audioInput=copy(value);
        applied=true;
      },{lineInputCommit:true});
      if(accepted===false||!applied||app().state.project!==s.project||changes.some(({track,...value})=>JSON.stringify(s.project.tracks[track].audioInput)!==JSON.stringify(value)))throw new Error('The input edit was not applied.');
      app().state.audio={...app().state.audio,...result};
      s.busy=false;app().closeModal();app().refresh();app().toast('Line inputs updated. Record a sample to keep the incoming audio.');
    } catch(error) {
      let text=error.message||String(error);
      if(attempted)try{await restore(s);}catch(failure){text+=` Could not restore input routing: ${failure.message||failure}`;}
      message(s,text);
    } finally {s.busy=false;controls(s);}
  }
  async function show() {
    if(opening||session?.busy||app().isChangingSession)return false;
    if(session && app().state.modal==='line-inputs'){get('line-input-apply')?.focus();return true;}
    const project=app().state.project, modal=app().state.modal;if(!project)return false;
    opening=true;let audio;
    try {audio=await app().request('audio.devices',{});}catch(error){app().toast(error.message||String(error),'error');return false;}finally{opening=false;}
    if(app().state.project!==project||app().state.modal!==modal)return false;
    const devices=(audio?.inputs||[]).filter(d=>Number(d.inputChannels)>0), selected=devices.find(d=>Number(d.id)===Number(audio.inputDevice||audio.defaultInput));
    const statuses=audio?.lineInputs||[];
    const s={project,activeTracks:new Set(statuses.filter(item=>item.active).map(item=>item.track)),pausedTracks:new Set(statuses.filter(item=>item.state==='paused').map(item=>item.track)),kept:new Set(statuses.filter(item=>item.state==='paused'||item.active).map(item=>item.track)),tracks:[...project.tracks],original:project.tracks.map(binding),devices,selectedUID:selected?.uid,rows:[],busy:false};session=s;
    const rows=project.tracks.map((track,index)=>{
      const original=s.original[index], value=original||{enabled:false,deviceUID:selected?.uid||'',channels:[0],gain:1};
      s.rows.push({track:index,original});
      const device=devices.find(d=>d.uid===value.deviceUID), count=Math.max(Number(device?.inputChannels)||0,...(value.channels||[]).map(c=>c+1),1);
      const options=Array.from({length:Math.min(256,count)},(_,i)=>`<option value="${i}">Input ${i+1}</option>`).join('');
      const status=routeStatus(audio,index), note=routeNote(status), via=status?.state==='live'&&status.via;
      const missing=value.deviceUID!==selected?.uid?`<option value="${esc(value.deviceUID||'')}">${value.deviceUID?(device?esc(device.name)+(via?' · via '+esc(status.via):status?.state==='paused'?' · paused':' · inactive'):'Saved interface · not connected'):'Choose interface'}</option>`:'';
      return `<fieldset class="field"><legend>${esc(track.name||'Track '+(index+1))}</legend><div class="row"><label><input id="line-input-enabled-${index}" type="checkbox" ${value.enabled?'checked':''}>Live input</label><label title="Hear this input through the track. Turn off when you listen through the interface instead."><input id="line-input-monitor-${index}" type="checkbox" ${value.monitor!==false?'checked':''}>Monitor</label><label>Interface <select id="line-input-device-${index}">${missing}${(selected?[selected]:[]).map(d=>`<option value="${esc(d.uid)}">${esc(d.name)}</option>`).join('')}</select></label><label>Left / mono <select id="line-input-left-${index}">${options}</select></label><label>Right <select id="line-input-right-${index}"><option value="mono">Mono</option>${options}</select></label><label>Gain <input id="line-input-gain-${index}" type="number" min="0" max="2" step="0.05" value="${value.gain??1}" style="width:70px"></label></div>${note?`<p class="line-input-lane-status" id="line-input-note-${index}">${esc(note)}</p>`:''}</fieldset>`;
    }).join('');
    app().openModal('Line inputs',`<p>Route mono or stereo interface inputs through track effects, sends and the mixer. Apply enables the chosen inputs. Current interface: ${esc(selected?.name||'none')}. Use Studio settings to change it.</p>${rows||'<p>Add an ordinary tracker lane to route an input.</p>'}<p id="line-input-status" role="status">${esc(audio?.lineInputError||'Choose channels, then Apply. Input audio is live until you record it.')}</p>`, '<button id="line-input-close">Cancel</button><button id="line-input-apply" class="primary">Apply inputs</button>',{kind:'line-inputs'});
    if(app().state.modal!=='line-inputs'){session=null;return false;}
    for(const row of s.rows){
      const index=row.track,v=row.original||{deviceUID:selected?.uid||'',channels:[0]};
      get(`line-input-device-${index}`).value=v.deviceUID||'';get(`line-input-left-${index}`).value=String(v.channels?.[0]??0);get(`line-input-right-${index}`).value=v.channels?.length===2?String(v.channels[1]):'mono';
      get(`line-input-device-${index}`).addEventListener('change',()=>{
        const device=devices.find(d=>d.uid===get(`line-input-device-${index}`).value), options=Array.from({length:Number(device?.inputChannels)||0},(_,i)=>`<option value="${i}">Input ${i+1}</option>`).join('');
        get(`line-input-left-${index}`).innerHTML=options;get(`line-input-left-${index}`).value='0';get(`line-input-right-${index}`).innerHTML='<option value="mono">Mono</option>'+options;get(`line-input-right-${index}`).value='mono';
      });
    }
    get('line-input-close').onclick=()=>{if(!s.busy)app().closeModal();};get('line-input-apply').onclick=()=>apply(s);
    get('line-input-apply').focus();return true;
  }
  function updateRecordingSummary(){
    const audio=app().state.audio||{},selected=(audio.inputs||[]).find(d=>Number(d.id)===Number(audio.inputDevice||audio.defaultInput));
    if(get('recording-interface-name'))get('recording-interface-name').textContent=selected?.name||'No interface selected';
    if(get('recording-interface-channels'))get('recording-interface-channels').textContent=selected?`${Number(selected.inputChannels)||0} input channel${Number(selected.inputChannels)===1?'':'s'}`:'Choose a device in the capture form';
  }
  function render() {
    const state=app().state,audio=state.audio||{},devices=audio.inputs||[],selected=devices.find(d=>Number(d.id)===Number(audio.inputDevice||audio.defaultInput));
    const lanes=state.project.tracks.map((track,index)=>{
      const input=track.audioInput;if(!input)return '';
      const device=devices.find(d=>d.uid===input.deviceUID),channels=(input.channels||[]).map(c=>c+1).join(' / '),status=routeStatus(audio,index),paused=input.enabled&&status?.state==='paused',live=input.enabled&&status?.state==='live',stopped=input.enabled&&!paused&&!live;
      return `<div class="recording-route"><span class="recording-route-dot ${live?'is-live':''}"></span><div><strong>${esc(track.name||'Track '+(index+1))}</strong><span>${esc(device?.name||'Interface unavailable')} · ${esc(channels)} ${(input.channels||[]).length===1?'mono':'stereo'}</span>${paused||status?.via?`<span class="line-input-lane-status">${esc(routeNote(status))}</span>`:stopped?`<span class="line-input-lane-status">${esc(audio.lineInputError||'Not running · check the audio interface in Settings')}</span>`:''}</div><small>${paused?'PAUSED':live?(input.monitor===false?'LIVE · MUTED':'LIVE'):stopped?'NOT RUNNING':'OFF'}</small></div>`;
    }).filter(Boolean).join('');
    return `<div class="recording-workspace"><aside class="sidebar recording-sidebar"><div class="sidebar-heading"><span class="eyebrow">Recording</span></div><button class="sidebar-item active" id="recording-capture-section"><span class="recording-sidebar-icon">●</span><div>Sample capture<div class="sub">Input or master output</div></div></button><button class="sidebar-item" id="recording-input-settings"><span class="recording-sidebar-icon">⇉</span><div>Live inputs<div class="sub">Route interface channels</div></div></button><div class="recording-sidebar-bottom"><span class="eyebrow">Capture destination</span><p>A new sampler + track.</p><span>Your existing sounds stay intact.</span><kbd>⌘⇧R</kbd><small>Start / stop recording</small></div></aside><section class="workspace-main recording-main"><div class="view-title"><div><div class="eyebrow">Capture &amp; sample</div><h1>Recording</h1></div><div class="recording-session-badge"><i></i> New sample</div></div><div class="recording-columns"><section class="panel recording-capture-panel" id="recording-setup"><div class="panel-head"><h2>Sample capture</h2><span class="recording-panel-note">Record · trim · play</span></div><div class="panel-body" id="recording-inline-host"><div class="recording-loading">Preparing capture controls…</div></div></section><aside class="recording-side-panels"><section class="panel"><div class="panel-head"><h2>Audio interface</h2><span class="recording-connection-dot"></span></div><div class="panel-body recording-interface"><strong id="recording-interface-name">${esc(selected?.name||'No interface selected')}</strong><span id="recording-interface-channels">${selected?`${Number(selected.inputChannels)||0} input channel${Number(selected.inputChannels)===1?'':'s'}`:'Choose a device in the capture form'}</span><button class="quiet" id="recording-studio-settings">Audio settings…</button></div></section><section class="panel" id="recording-routing"><div class="panel-head"><h2>Live input routing</h2><button class="quiet" id="recording-routing-configure">Configure</button></div><div class="panel-body recording-routes">${lanes||'<div class="recording-empty-routes"><span>⇉</span><strong>No live lanes</strong><p>Send separate input channels through track effects and the mixer.</p></div>'}</div><div class="recording-panel-foot">Live lanes provide monitoring through their mixer channels.</div></section><section class="recording-capture-note"><span class="eyebrow">After capture</span><p><strong>Stop &amp; load</strong> opens your new sampler, ready to trim and play. Choose the named replacement destination only when you want to overwrite a sound.</p></section></aside></div></section></div>`;
  }
  function bind() {
    get('recording-capture-section').onclick=()=>get('record-name')?.focus();
    get('recording-input-settings').onclick=show;
    get('recording-routing-configure').onclick=show;
    get('recording-studio-settings').onclick=()=>app().perform('settings');
    window.STRecording?.mountInline?.();
  }
  /* Mixer: the selected track's live input, edited in place (Live input on/off, left and right channel of the
     current Studio interface). Each change is applied to the audio host first, then committed as one Undo step. */
  let mixerBusy=false;
  function mixerSection(index){
    const state=app().state,track=state.project?.tracks?.[index];if(!track)return '';
    const audio=state.audio||{},devices=(audio.inputs||[]).filter(d=>Number(d.inputChannels)>0),selected=devices.find(d=>Number(d.id)===Number(audio.inputDevice||audio.defaultInput));
    const input=track.audioInput||{enabled:false,deviceUID:selected?.uid||'',channels:[0],gain:1},status=routeStatus(audio,index);
    const device=devices.find(d=>d.uid===input.deviceUID)||selected,count=Math.max(Number(device?.inputChannels)||0,...(input.channels||[]).map(c=>c+1),1);
    const option=(value,label,current)=>`<option value="${value}" ${String(value)===String(current)?'selected':''}>${esc(label)}</option>`;
    const inputs=Array.from({length:Math.min(256,count)},(_,i)=>i);
    const live=input.enabled&&status?.state==='live',paused=input.enabled&&status?.state==='paused';
    const state_=!input.enabled?'Off':live?(input.monitor===false?'Live · not monitored':'Live'):paused?'Paused':'Not running';
    const note=paused?routeNote(status):input.enabled&&!live?(audio.lineInputError||'Check the audio interface in Settings'):'';
    const other=input.deviceUID&&selected&&input.deviceUID!==selected.uid;
    return `<div class="routing-subhead"><h4>Live input</h4><span class="line-input-mixer-state${live?' is-live':''}">${state_}</span></div>`
      +`<div class="line-input-mixer" data-line-input-mixer="${index}"><label><input type="checkbox" data-line-input-field="enabled" ${input.enabled?'checked':''} ${mixerBusy||!selected?'disabled':''}>Live input from ${esc(device?.name||'interface')}</label>`
      +`<label title="Hear this input through the track. Turn off when you listen through the interface instead."><input type="checkbox" data-line-input-field="monitor" ${input.monitor!==false?'checked':''} ${mixerBusy||!selected||!input.enabled?'disabled':''}>Monitor</label>`
      +`<label>Left / mono<select data-line-input-field="left" ${mixerBusy||!selected?'disabled':''}>${inputs.map(i=>option(i,`Input ${i+1}`,input.channels?.[0]??0)).join('')}</select></label>`
      +`<label>Right<select data-line-input-field="right" ${mixerBusy||!selected?'disabled':''}>${option('mono','Mono',input.channels?.length===2?'':'mono')}${inputs.map(i=>option(i,`Input ${i+1}`,input.channels?.length===2?input.channels[1]:'')).join('')}</select></label></div>`
      +(other?`<p class="routing-description">Saved for ${esc(devices.find(d=>d.uid===input.deviceUID)?.name||'another interface')}; changing it here moves it to ${esc(selected.name)}.</p>`:'')
      +(note?`<p class="routing-description">${esc(note)}</p>`:'')
      +(!selected?'<p class="routing-description">Choose an audio interface with inputs in Settings (⌘,).</p>':'');
  }
  async function applyMixer(index,root){
    if(mixerBusy||app().isChangingSession)return false;
    const state=app().state,project=state.project,track=project?.tracks?.[index];if(!track)return false;
    const audio=state.audio||{},selected=(audio.inputs||[]).find(d=>Number(d.id)===Number(audio.inputDevice||audio.defaultInput));
    if(!selected){app().toast('Choose an audio interface with inputs in Settings first.','error');return false;}
    const field=name=>root.querySelector(`[data-line-input-field="${name}"]`);
    const left=Number(field('left').value),right=field('right').value,channels=right==='mono'||Number(right)===left?[left]:[left,Number(right)];
    const value=withMonitor({...(track.audioInput||{gain:1}),enabled:field('enabled').checked,deviceUID:selected.uid,channels,gain:track.audioInput?.gain??1},field('monitor')?.checked!==false);
    mixerBusy=true;let attempted=false;
    try{
      if(await app().captureNativeEdits?.()===false)throw new Error('Could not preserve current instrument settings. Try again.');
      await app().waitForGraphEdit?.();await app().waitForEngineUpdate?.();
      if(app().state.project!==project||project.tracks[index]!==track)throw new Error('The project changed. Try again.');
      attempted=true;
      const result=await app().request('audio.inputs',{inputs:[{track:index,...value}]},{lineInputCommit:true});
      if(!result||!Array.isArray(result.lineInputs))throw new Error('The audio host did not confirm input routing.');
      // The track list can change during the native round trip (AI control, MK3): never write the route onto the track that moved into this index.
      if(app().state.project!==project||project.tracks[index]!==track)throw new Error('The project changed while applying the input. Try again.');
      const label=!value.enabled?'Live input off':track.audioInput?.enabled&&(track.audioInput.monitor!==false)!==(value.monitor!==false)?(value.monitor===false?'Monitor off':'Monitor on'):'Live input';
      const accepted=app().mutate(label,q=>{q.tracks[index].audioInput=copy(value);},{lineInputCommit:true});
      if(accepted===false)throw new Error('The input edit was not applied.');
      app().state.audio={...app().state.audio,...result};
      return true;
    }catch(error){
      if(attempted)try{await app().request('audio.inputs',{inputs:[{track:index,...(binding(project.tracks[index])||{enabled:false})}]},{lineInputCommit:true});}catch{}
      app().toast(error.message||String(error),'error');return false;
    }finally{mixerBusy=false;app().refresh();}
  }
  document.addEventListener?.('change',event=>{
    const root=event.target?.closest?.('[data-line-input-mixer]');
    if(root&&event.target.matches('[data-line-input-field]'))applyMixer(Number(root.dataset.lineInputMixer),root);
  });

  window.STLineInputs={show,render,bind,updateRecordingSummary,mixerSection,applyMixer,closed(){if(session?.busy||opening)return false;session=null;return true;},get isBusy(){return opening||!!session?.busy;}};
})();
