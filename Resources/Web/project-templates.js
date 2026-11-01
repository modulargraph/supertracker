/* Reusable project templates are independent of the currently open document. */
(() => {
  const S=()=>window.ST;
  let busy=false,generation=0;
  async function show(){
    if(busy)return false;
    const token = ++generation,
      app = S(),
      project = app.state.project,
      initialModal = app.state.modal,
      initialMarkup = document.getElementById('overlay')?.innerHTML;
    try{
      const data=await app.request('project.templates.list');
      if(token!==generation
          ||app.state.project!==project
          ||app.state.modal!==initialModal
          ||document.getElementById('overlay')?.innerHTML!==initialMarkup
          ||app.isChangingSession)return false;
      const templates=data.templates||[],esc=app.escapeHTML;
      app.openModal('Project templates', `<p>New projects use the selected default. Existing projects are unchanged.</p><label>Default for new projects <select id="template-default">`
          + `<option value="">Blank project</option>${templates.map(t => `<option value="${esc(t.id)}" ${t.id === data.defaultID ? 'selected' : ''}>${esc(t.name)}</option>`).join('')}</select>`
          + `</label><div class="stack">${templates.map(t => `<div class="row"><span class="grow">${esc(t.name)}</span><button data-template-new="${esc(t.id)}">New from template</button>`
          + `</div>`).join('')}</div>`
          + `<hr><label>Save current project as a reusable template <input id="template-name" value="${esc(project.name)}" maxlength="120">`
          + `</label><p id="template-status" role="status"></p>`, '<button id="template-close">Close</button><button id="template-save">Save template</button>', {
        kind: 'project-templates'
      });
      if(app.state.modal!=='project-templates')return false;
      const get=id=>document.getElementById(id),saveButton=get('template-save'),status=get('template-status');
      const owns=()=>token===generation&&app.state.project===project&&app.state.modal==='project-templates'&&get('template-save')===saveButton;
      const report=error=>{if(owns())status.textContent=error.message||String(error);};
      let acknowledgedDefault=data.defaultID||'';
      get('template-close').onclick=()=>{if(owns()){generation++;app.closeModal();}};
      get('template-default').onchange=async event=>{
        if(busy||!owns())return;busy=true;const field=event.target,value=field.value;field.disabled=true;
        try{await app.request('project.templates.setDefault',
            {id:value||null});acknowledgedDefault=value;if(owns())app.toast('Default for new projects updated');}
        catch(error){field.value=acknowledgedDefault;report(error);}finally{busy=false;field.disabled=false;}
      };
      document.querySelectorAll('[data-template-new]').forEach(button => button.onclick = () => {
        if (busy || !owns()) return;
        generation++;
        app.closeModal();
        return app.projectAction('new', button.dataset.templateNew);
      });
      get('template-save').onclick=async()=>{
        if(busy||!owns())return;const name=get('template-name').value.trim();if(!name){report(Error('Enter a template name.'));return;}
        busy=true;saveButton.disabled=true;let saved=false;
        try{if(await app.captureNativeEdits()===false)throw Error('Could not capture the current project.');if(!owns())return;await app.request('project.templates.save',
            {name,project:window.STUtil.clone(project)});saved=true;}
        catch(error){report(error);}finally{busy=false;saveButton.disabled=false;}
        if(saved&&owns()){app.toast('Reusable template saved');await show();}
      };return true;
    }catch(error){if(token===generation&&app.state.project===project&&app.state.modal===initialModal)app.toast(error.message,'error');return false;}
  }
  window.STProjectTemplates={show,get isBusy(){return busy;}};
})();
