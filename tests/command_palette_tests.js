'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const web = require('./helpers/web');
function harness(){
 const nodes=new Map(),performed=[];let items=[],closed=0,modal;
 function node(id){const attributes={};let html='';const element={id,value:'',attributes,setAttribute(name,value){attributes[name]=String(value);},removeAttribute(name){delete attributes[name];},scrollIntoView(){this.scrolled=true;}};
  Object.defineProperty(element,'innerHTML',{get:()=>html,set(value){html=value;if(id==='palette-list'){items=[];for(const [,attrs] of value.matchAll(/<button\b([^>]*)>/g)){const item=node(attrs.match(/id="([^"]+)"/)[1]);item.dataset={command:attrs.match(/data-command="([^"]+)"/)[1]};item.active=/class="[^"]*\bactive\b/.test(attrs);item.selected=attrs.match(/aria-selected="([^"]+)"/)[1];item.tabIndex=Number(attrs.match(/tabindex="([^"]+)"/)[1]);items.push(item);}}}});nodes.set(id,element);return element;
 }
 const stubs={esc:value=>String(value),openModal(title,html,footer,config){modal={title,html,config};for(const id of ['palette-search','palette-list'])node(id);},closeModal(){closed++;},perform:command=>performed.push(command),$:selector=>selector==='.palette-item.active'?items.find(item=>item.active):nodes.get(selector.slice(1)),$$:()=>items};
 const context=vm.createContext({window:{},String,Set,Math,Number});
 const app=web.loadAppWith(context,stubs);app.openPalette();
 const search=nodes.get('palette-search');
 return {context,app,search,performed,get modal(){return modal;},get closed(){return closed;},get items(){return items;},ids:query=>Array.from(app.searchCommands(query),c=>c[0]),query(value){search.value=value;search.oninput();},key(key,extra={}){const event={key,preventDefault(){this.prevented=true;},...extra};search.onkeydown(event);return event;},active(){return items.find(item=>item.active);},list:()=>nodes.get('palette-list')};
}
{
 const html=fs.readFileSync('ui/index.html','utf8');
 assert(!/maschine-chordsets/.test(html),'production page loads no imported chord catalogue');
 const h=harness();
 h.query('chord set');
 assert.equal(h.items.filter(item=>item.dataset.command.startsWith('chordSet:')).length,31,'all harmony and style chord sets are rendered');
 assert.equal(h.items.filter(item=>item.selected==='true').length,1,'selection is not the full result list');
 for(const [query,id] of [['Major triads','major-triads'],['Minor triads','minor-triads'],['Harmonic minor','minor-harmonic'],['dorian','minor-dorian'],['Chord set: Lydian float','major-lydian'],['secondary dom','major-secondary'],['neo soul','neosoul']]){
  h.query(query);assert.equal(h.active().dataset.command,'chordSet:'+id,query+' ranks the named set first');
  h.key('Enter');assert.equal(h.performed.at(-1),'chordSet:'+id,query+' activates the named set');
 }
 console.log('PASS chord-set registration: all 31 sets render; set names select and dispatch the named set');
}
{
 const h=harness();
 assert.equal(h.app.commandSearchScore('Sample sampler record','sam sample record',['sam','sample','record']),30,'longer prefixes match before consuming a shared short prefix');
 assert.equal(h.app.commandSearchScore('Sample record','sam sample record',['sam','sample','record']),Infinity,'one label token cannot satisfy two query terms');
 console.log('PASS repeated tokens and unordered prefixes stay distinct');
}
{
 const h=harness();
 for(const query of ['duplicate track','track duplicate','  DUPLICATE   TRACK  ','tráck—dupl','dupl tra'])assert.equal(h.ids(query)[0],'duplicateTrack',query);
 assert.equal(h.ids('pattern duplicate')[0],'duplicatePattern');
 assert.equal(h.ids('sample record')[0],'recordInput');assert.equal(h.ids('export stems')[0],'export');
 assert.deepEqual(h.ids('duplicate track unknown'),[]);assert.deepEqual(h.ids('duplicate instrument'),[]);
 console.log('PASS normalized words in any order and word prefixes find commands without requiring adjacent labels');
}
{
 const h=harness();assert.deepEqual(h.ids('save project').slice(0,2),['save','saveAs']);assert.equal(h.ids('stop')[0],'stop');assert.equal(h.ids('play stop')[0],'play');
 assert.equal(h.ids('import sample')[0],'loadSample');assert.equal(h.ids('record synth')[0],'recordInput');assert.equal(h.ids('line in record')[0],'recordInput');assert.equal(h.ids('multiple inputs')[0],'lineInputs');assert.equal(h.ids('render stems')[0],'export');assert.equal(h.ids('export audio')[0],'export');
 // Label matches outrank aliases, and equal scores retain catalogue order.
 assert.equal(h.ids('audio')[0],'settings');const all=h.ids('');assert.deepEqual(h.ids('  — /  '),all);assert.deepEqual(h.ids('track'),['bounceTrack','addTrack','duplicateTrack','renameTrack','freezeTrack','unfreezeTrack','ungroupTrack','addGroupTrack','moveTrackLeft','moveTrackRight','toggleTrackCollapsed','addNoteColumn','removeNoteColumn','assignInstrument','muteTrack','soloTrack','automateParameter','lineInputs','groupTracks','dissolveGroup','hideEmptyTracks','hideEmptyScope','expandAllTracks','deleteTracks','deleteGroup','padKeys','clearMuteSolo']);assert(h.ids('track').every(id=>all.includes(id)));assert.deepEqual(h.ids('track'),h.ids('track'));
 console.log('PASS exact/prefix label ranking, narrow workflow aliases, and stable catalogue ties');
}
{
 const h=harness();assert.match(h.modal.html,/role="combobox"/);assert.match(h.modal.html,/aria-controls="palette-list"/);assert.match(h.modal.html,/aria-autocomplete="list"/);assert.match(h.modal.html,/aria-expanded="true"/);
 h.query('track');const first=h.active().id;assert.equal(h.search.attributes['aria-activedescendant'],first);assert(h.items.every(item=>item.tabIndex===-1));assert.equal(h.items.filter(item=>item.selected==='true').length,1);
 assert.equal(h.key('ArrowDown').prevented,true);assert.equal(h.active().dataset.command,'addTrack');assert.equal(h.search.attributes['aria-activedescendant'],h.active().id);
 h.key('ArrowUp');assert.equal(h.active().id,first);h.key('ArrowUp');assert.equal(h.active().id,first);
 for(let i=0;i<30;i++)h.key('ArrowDown');assert.equal(h.active().dataset.command,'clearMuteSolo');h.key('Enter');assert.deepEqual(h.performed,['clearMuteSolo']);assert.equal(h.closed,1);
 console.log('PASS arrows stay bounded and combobox active descendant always identifies the sole selected option');
}
{
 const h=harness();h.query('track');h.key('ArrowDown');h.query('duplicate track');assert.equal(h.active().dataset.command,'duplicateTrack');assert.equal(h.search.attributes['aria-activedescendant'],h.active().id);
 h.query('nothing matches this');assert.equal(h.items.length,0);assert.equal(h.search.attributes['aria-activedescendant'],undefined);assert.match(h.list().innerHTML,/No matching command/);h.key('ArrowDown');h.key('ArrowUp');h.key('Enter');assert.deepEqual(h.performed,[]);assert.equal(h.closed,0);
 h.query('duplicate track');h.key('Enter');assert.deepEqual(h.performed,['duplicateTrack']);
 console.log('PASS changed queries reset selection; empty navigation and Enter are inert and recover correctly');
}
{
 const h=harness();h.query('track');const first=h.active().id;
 for(const extra of [{isComposing:true},{keyCode:229},{altKey:true},{metaKey:true},{ctrlKey:true},{shiftKey:true}]){assert.equal(h.key('ArrowDown',extra).prevented,undefined);assert.equal(h.key('Enter',extra).prevented,undefined);}
 assert.equal(h.active().id,first);assert.deepEqual(h.performed,[]);
 const target=h.items[2],event={preventDefault(){this.prevented=true;}};target.onmousedown(event);assert.equal(event.prevented,true);target.onclick();assert.deepEqual(h.performed,[target.dataset.command]);
 console.log('PASS IME and modified text keys do not launch commands; pointer activation remains available');
}
