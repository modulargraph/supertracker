/* Explicit pattern destination: navigate existing tracks, then choose the track's instrument. */
(() => {
  'use strict';
  const S=()=>window.ST,state=()=>S().state,esc=x=>S().escapeHTML(String(x??''));
  let assigning=null,arming=null,playing=null;
  const grid=()=>{const node=document.getElementById('tracker');node?.focus({preventScroll:true});};
  function laneLabel(lane,index,p){
    const group=window.STGroups.ofTrack(p,index);
    return `${String(index+1).padStart(2,'0')} · ${lane.name} · ${group?'Group '+group.name:p.instruments[lane.instrument]?.name||'Instrument'}`;
  }
  function instruction(){
    if(window.STGroups.ofTrack(state().project,state().track)&&state().padKeys)return 'Pad keys: Z S X D C V … play and enter the group’s tracks on this row. ⌥Z turns them off.';
    return 'Arm notes, then type Z–M on the grid. ↑↓ chooses a step; Space plays and Escape stops. Existing notes keep their instrument numbers.';
  }
  function render(){
    const st=state(),p=st.project,lane=p.tracks[st.track];
    if(!lane)return '';
    return `<section class="pattern-destination is-instrument" aria-label="Pattern note destination">
      <div class="pattern-destination-controls"><label for="pattern-destination-lane"><span>Selected track</span><select id="pattern-destination-lane">${p.tracks.map((t,i)=>`<option value="${i}" ${i===st.track?'selected':''}>${esc(laneLabel(t,i,p))}</option>`).join('')}</select></label>
      <label for="pattern-destination-instrument"><span>Assigned instrument</span><select id="pattern-destination-instrument" ${assigning?'disabled':''}>${p.instruments.map((i,n)=>`<option value="${n}" ${n===lane.instrument?'selected':''}>${String(n).padStart(2,'0')} · ${esc(i.name)}</option>`).join('')}</select></label>
      <label for="pattern-record-placement"><span>Record placement</span><select id="pattern-record-placement" title="Selected step holds the editing cursor during playback; Live playhead records transport timing"><option value="selected" ${st.recordPlacement!=='live'?'selected':''}>Selected step</option><option value="live" ${st.recordPlacement==='live'?'selected':''}>Live playhead</option></select></label><button id="pattern-enter-notes" title="Arm note recording and focus the grid · ⌘R toggles note recording">Arm & enter notes <kbd>⌘R</kbd></button><button id="pattern-play-current" title="Play this pattern from the cursor, independently of Song mode">Play pattern</button><button id="pattern-line-inputs" title="Route hardware input channels to mixer tracks">Line inputs…</button><button id="pattern-record-sample" title="Record audio to a sample or clip · ⌘⇧R">Record sample…</button></div>
      <p id="pattern-destination-help">${esc(instruction())}</p></section>`;
  }
  function update(){
    const arm=document.getElementById('pattern-enter-notes'),play=document.getElementById('pattern-play-current'),help=document.getElementById('pattern-destination-help');
    if(arm){arm.disabled=!!arming;arm.textContent=arming?'Arming notes…':state().recording?'Enter notes · armed':'Arm & enter notes ⌘R';arm.setAttribute('aria-pressed',String(state().recording));}
    if(play){play.textContent=playing?'Cancel preparing':state().playing?'Stop playback':'Play pattern';play.setAttribute('aria-busy',String(!!playing));}
    if(help)help.textContent=assigning?'Assigning instrument…':instruction();
  }
  function owned(project,lane){return state().project===project&&project.tracks[state().track]===lane&&state().view==='pattern'&&!state().modal;}
  function refreshFocus(id,project,lane,fallback=false){if(!owned(project,lane))return;const focus=document.activeElement,restore=focus?.id===id||fallback&&(!focus||focus===document.body);S().refresh();if(restore)document.getElementById(id)?.focus({preventScroll:true});}
  async function assign(index,project,lane){
    if(assigning||!owned(project,lane)||!Number.isInteger(index)||!project.instruments[index])return false;
    if(lane.instrument===index)return true;
    const prior=state().instrument,returnFocus=document.activeElement?.id==='pattern-destination-instrument',operation={project,lane};assigning=operation;S().stopHeld();state().instrument=index;update();
    const field=document.getElementById('pattern-destination-instrument');if(field)field.disabled=true;
    try{return await S().perform('assignInstrument');}
    finally{
      if(assigning===operation)assigning=null;
      if(owned(project,lane)&&lane.instrument!==index&&state().instrument===index)state().instrument=prior;
      refreshFocus('pattern-destination-instrument',project,lane,returnFocus);update();
    }
  }
  function bind(){
    const project=state().project,lane=project.tracks[state().track],tracks=project.tracks.slice();
    const on=(id,fn)=>{const e=document.getElementById(id);if(e)e.onchange=fn;};
    on('pattern-record-placement',e=>{S().setRecordPlacement(e.target.value);grid();});
    on('pattern-destination-lane',e=>{const index=Number(e.target.value);if(!owned(project,lane)||!Number.isInteger(index)||!tracks[index]||project.tracks[index]!==tracks[index])return;
      S().stopHeld();state().track=index;state().instrument=tracks[index].instrument||0;state().selection=null;S().refresh();document.getElementById('pattern-destination-lane')?.focus({preventScroll:true});});
    on('pattern-destination-instrument',e=>assign(Number(e.target.value),project,lane));
    for(const id of ['pattern-destination-lane','pattern-destination-instrument']){
      const el=document.getElementById(id);if(el)el.onkeydown=e=>{if((e.key==='Enter'||e.key==='Escape')&&!e.metaKey&&!e.ctrlKey&&!e.altKey){e.preventDefault();e.stopPropagation();if(!assigning)grid();}};
    }
    const arm=document.getElementById('pattern-enter-notes');if(arm)arm.onclick=async()=>{
      if(arming||!owned(project,lane))return false;const operation={project,lane},invoker=document.activeElement;arming=operation;update();
      try{const accepted=await S().setNoteRecording(true);if(accepted&&owned(project,lane)&&(document.activeElement===invoker||document.activeElement===document.body&&invoker?.id==='pattern-enter-notes'||['pattern-enter-notes','tracker'].includes(document.activeElement?.id)))grid();return accepted;}
      finally{if(arming===operation)arming=null;update();}
    };
    const play=document.getElementById('pattern-play-current');if(play)play.onclick=async()=>{
      if(!owned(project,lane))return false;
      if(playing||state().playing)return S().perform('stop');
      if(!S().perform('modePattern'))return false;grid();
      const operation={project};playing=operation;update();
      try{return await S().perform('play');}finally{if(playing===operation)playing=null;update();}
    };
    for(const [id,command] of [['pattern-line-inputs','lineInputs'],['pattern-record-sample','recordInput']]){const button=document.getElementById(id);if(button)button.onclick=()=>S().perform(command);}
    update();
  }
  window.STPatternDestination={render,bind,update};
})();
