/* Explicit intentional plug-in baseline capture, separate from automation playback. */
(() => {
  'use strict';
  const S=()=>window.ST;
  let busy=false;
  // Readiness from the last check lives beside the plug-in object, outside the project, so a
  // failed check never edits the session behind undo. Undo/replacement drops it with the object.
  const checked=new WeakMap();
  const patchOf=plugin=>plugin&&checked.has(plugin)?{...plugin.basePatch,...checked.get(plugin)}:plugin?.basePatch;
  const sameTarget = (a, b) => a?.instanceID === b?.instanceID
      && a?.kind === b?.kind
      && (a?.slot ?? 0) === (b?.slot ?? 0)
      && (a?.owner ? a.owner === b?.owner : !b?.owner && a?.track === b?.track)
      && a?.component === b?.component;
  const find=(project,id)=>project.audioPlugins?.find(p=>p.instanceID===id);
  function render(plugin){
    const patch=patchOf(plugin);
    if(patch?.supported!==true)return '';
    const unavailable=patch.captureReady!==true;
    return `<button data-capture-base="${S().escapeHTML(plugin.instanceID)}" ${busy?'disabled':''} title="Capture the current plug-in patch as the `
        + `saved starting sound. Automation lanes remain unchanged.">${unavailable?'Check & update base patch':'Update base patch'}</button>`
        + `<span class="small-note">${unavailable
            ?'Get a patch in the editor, then check capture readiness'
            :patch.manualEdits
            ?'Manual edits update the base automatically'
            :'Base patch retained separately from automation'}</span>`;
  }
  const blocked=()=>S().state.playing||window.STBusy.any(['session', 'recording', 'export', 'automationWrite', 'lineInputs']);
  async function capture(id){
    const app=S(),project=app.state.project,target=find(project,id);
    if(busy||target?.basePatch?.supported!==true)return false;
    if(blocked()){app.toast('Stop playback and finish the current edit before updating the base patch.');return false;}
    busy=true;bind();
    try{
      if(await app.captureNativeEdits()===false)return false;
      await app.waitForGraphEdit?.();await app.waitForEngineUpdate?.();
      if (blocked()
          || app.state.project !== project
          || !sameTarget(target, find(project, id))) throw Error('The plug-in changed. Select it again before updating its base patch.');
      const selector={...(target.owner?{owner:target.owner}:{track:target.track}),kind:target.kind,slot:target.slot??0,instanceID:id};
      const fresh=await app.request('plugin.state',selector);
      if (blocked()
          || app.state.project !== project
          || !sameTarget(target, find(project, id))
          || !sameTarget(target, fresh)) throw Error('The plug-in changed. Select it again before updating its base patch.');
      // Read readiness from the live editor after Get, without adopting its live
      // parameters or replacing the saved baseline as a side effect of checking.
      if(fresh.stateError)throw Error(String(fresh.stateError));
      if(!fresh.basePatch)throw Error('The plug-in returned no capture capability. Try checking again.');
      checked.set(find(project,id),fresh.basePatch);
      if (fresh.basePatch?.supported !== true
          || fresh.basePatch.captureReady !== true) throw Error('Get a patch in the plug-in editor first, then choose Check & update base patch again.');
      const result=await app.request('plugin.captureBasePatch',selector);
      if (blocked()
          || app.state.project !== project
          || !sameTarget(target, find(project, id))
          || !sameTarget(target, result)
          || result?.basePatch?.supported !== true
          || typeof result.state !== 'string'
          || !result.state) throw Error('The plug-in returned no matching base patch. The project was not changed.');
      let applied=false;
      app.mutate('Update base patch', p => {
        const index = p.audioPlugins.findIndex(item => item.instanceID === id);
        if (index < 0 || !sameTarget(target, p.audioPlugins[index])) return;
        p.audioPlugins[index] = { ...p.audioPlugins[index], ...result };
        if (target.kind === 'instrument') {
          for (const instrument of p.instruments || []) if (instrument?.plugin?.instanceID === id) instrument.plugin = window.STUtil.clone(p.audioPlugins[index]);
        }
        applied = true;
      }, { basePatchCommit: true });
      if(!applied)throw Error('The base patch could not be stored. Try again after the current edit finishes.');
      checked.delete(target);
      app.toast('Base patch updated. Automation lanes are unchanged.','success');return true;
    }catch(error){app.toast(error.message||String(error),'error');return false;}
    finally{busy=false;bind();}
  }
  function bind() {
    for (const button of document.querySelectorAll('[data-capture-base]')) {
      const patch = patchOf(find(S().state.project, button.dataset.captureBase));
      button.disabled = busy || patch?.supported !== true;
      button.textContent = patch?.captureReady === true ? 'Update base patch' : 'Check & update base patch';
      button.onclick = () => capture(button.dataset.captureBase);
    }
  }
  window.STBasePatch={render,bind,capture,get isBusy(){return busy;}};
})();
