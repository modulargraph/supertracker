'use strict';
// Focused DOM adapter: real navigation module, labelled form nodes and modal focus.
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const web = require('./helpers/web');
const source = web.source('instrument-navigation.js');
const escapeHTML=value=>String(value).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
function harness(){
  let document,origin,modal;const notices=[],state={view:'instrument',instrument:0,track:0,modal:null,project:{instruments:[{name:'Fixture'}]}};
  class Element{
    constructor(tag,attrs={},text='',children=[]){this.tagName=tag.toUpperCase();this.attrs={...attrs};this.text=text;this.children=[];this.dataset={};this.style={};this.value='';this.isConnected=true;for(const [key,value] of Object.entries(attrs))if(key.startsWith('data-'))this.dataset[key.slice(5).replace(/-([a-z])/g,(_,c)=>c.toUpperCase())]=value;children.forEach(child=>this.append(child));this.classList={toggle:(name,on)=>{const names=new Set((this.attrs.class||'').split(' '));on?names.add(name):names.delete(name);this.attrs.class=[...names].join(' ');}};}
    get id(){return this.attrs.id||'';}get type(){return this.attrs.type||'';}get disabled(){return Object.hasOwn(this.attrs,'disabled');}get textContent(){return this.text+this.children.map(child=>child.textContent).join(' ');}set textContent(value){this.text=String(value);this.children=[];}
    get labels(){return document.body.querySelectorAll('label').filter(label=>label.attrs.for===this.id||label.contains(this));}
    get selectedOptions(){return this.tagName==='SELECT'?this.children.filter(child=>Object.hasOwn(child.attrs,'selected')):[];}
    append(child){child.parentElement=this;this.children.push(child);return child;}contains(node){return node===this||this.children.some(child=>child.contains(node));}
    getAttribute(name){return this.attrs[name]??null;}setAttribute(name,value){this.attrs[name]=String(value);}removeAttribute(name){delete this.attrs[name];}
    matches(selector){return selector.split(',').some(part=>{part=part.trim();const ancestor=part.lastIndexOf(' ');if(ancestor>=0)return this.matches(part.slice(ancestor+1))&&!!this.parentElement?.closest(part.slice(0,ancestor));const tag=/^[a-z][\w-]*/i.exec(part)?.[0];if(tag&&this.tagName!==tag.toUpperCase())return false;const id=/#([\w-]+)/.exec(part)?.[1];if(id&&this.id!==id)return false;for(const [,name]of part.matchAll(/\.([\w-]+)/g))if(!(this.attrs.class||'').split(' ').includes(name))return false;for(const [,name,value]of part.matchAll(/\[([^=\]]+)(?:="([^"]*)")?\]/g)){if(!Object.hasOwn(this.attrs,name)||value!==undefined&&this.attrs[name]!==value)return false;}return true;});}
    closest(selector){for(let node=this;node;node=node.parentElement)if(node.matches(selector))return node;return null;}
    querySelectorAll(selector){return this.children.flatMap(child=>[...(child.matches(selector)?[child]:[]),...child.querySelectorAll(selector)]);}querySelector(selector){return this.querySelectorAll(selector)[0]||null;}
    cloneNode(deep){return new Element(this.tagName,this.attrs,this.text,deep?this.children.map(child=>child.cloneNode(true)):[]);}
    remove(){this.isConnected=false;if(this.parentElement)this.parentElement.children=this.parentElement.children.filter(child=>child!==this);for(const child of this.children)child.isConnected=false;}
    focus(){if(!this.disabled&&this.isConnected)document.activeElement=this;}select(){this.selected=true;}scrollIntoView(options){this.scrolled=options;}
    set innerHTML(value){this.html=value;this.children.forEach(child=>child.remove());this.children=[];for(const match of value.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)){const attrs={};for(const [,key,val]of match[1].matchAll(/([\w-]+)="([^"]*)"/g))attrs[key]=val;this.append(new Element('button',attrs,match[2].replace(/<[^>]+>/g,'')));}}get innerHTML(){return this.html||'';}
    fire(type,values={}){const event={target:this,preventDefault(){this.defaultPrevented=true;},stopPropagation(){this.stopped=true;},...values};this['on'+type]?.(event);return event;}
  }
  const n=(tag,attrs={},text='',children=[])=>new Element(tag,attrs,text,children),body=n('body'),main=body.append(n('main',{id:'main'})),workspace=main.append(n('section',{class:'workspace-main'})),invoker=workspace.append(n('button',{id:'invoker'},'Rename'));
  document={body,activeElement:invoker,getElementById:id=>body.querySelectorAll('[id]').find(node=>node.id===id)||null,querySelector:selector=>body.querySelector(selector),querySelectorAll:selector=>body.querySelectorAll(selector)};
  const ST={state,escapeHTML,toast:message=>notices.push(message),request(){throw Error('Navigation must not request audio');},mutate(){throw Error('Navigation must not mutate sound');},openModal(title,bodyHTML,footer,options){origin=document.activeElement;state.modal=options.kind;modal=body.append(n('div',{id:'overlay'}));for(const [,tag,attrs]of bodyHTML.matchAll(/<(input|div|span)\b([^>]*)>/g)){const properties={};for(const [,key,value]of attrs.matchAll(/([\w-]+)="([^"]*)"/g))properties[key]=value;if(properties.id)modal.append(n(tag,properties));}},closeModal(){state.modal=null;modal?.remove();origin?.focus();}};
  const window={ST,getComputedStyle:node=>node.style};web.runText('instrument-navigation.js', source, {window,document});
  const get=id=>document.getElementById(id),search=value=>{get('instrument-find-search').value=value;get('instrument-find-search').fire('input');return get('instrument-find-results').innerHTML;};
  const panel=(title,children,attrs={})=>workspace.append(n('section',{class:'panel',...attrs},'',[n('h2',{},title),...children]));
  const field=(label,id,attrs={})=>n('label',{},label,[n('input',{id,type:'number',...attrs})]);
  return {ui:window.STInstrumentNavigation,ST,state,document,workspace,notices,n,panel,field,get,search,invoker,key:(key,extra={})=>get('instrument-find-search').fire('keydown',{key,...extra}),results:()=>get('instrument-find-results').querySelectorAll('[data-instrument-jump]')};
}
const tests=[],test=(name,run)=>tests.push([name,run]);
test('search finds a Slice map section and jumps to its selected slice without auditioning',()=>{
  const h=harness(),selected=h.n('button',{id:'slice-1','data-sampler-audition':'0',tabindex:'0','aria-label':'Audition slice 1'},'▶');let played=0;selected.onclick=()=>played++;
  h.panel('Slice map',[h.n('button',{id:'detect'},'Detect transients'),selected]);assert(h.ui.show());assert.equal(h.document.activeElement.id,'instrument-find-search');assert.match(h.search('slice map'),/Slice map/);
  const event=h.key('Enter');assert(event.defaultPrevented&&event.stopped);assert.equal(h.state.modal,null);assert.equal(h.document.activeElement,selected);assert.equal(selected.scrolled.block,'center');assert.equal(played,0);
});
test('controls on a hidden instrument page are found with their page name and the jump opens that page',()=>{
  const h=harness();h.state.project.instruments[0].type='sample';
  const page=h.workspace.append(h.n('div',{id:'inst-page-voice','data-inst-page':'voice','data-inst-page-label':'Voice & Record',hidden:''}));
  const choke=h.n('select',{id:'sampler-choke','aria-label':'Choke group'});page.append(h.n('section',{class:'panel'},'',[h.n('h2',{},'Voice behavior'),choke]));
  h.ui.show();assert.match(h.search('choke'),/Voice &amp; Record · Voice behavior/);h.key('Enter');
  assert.equal(page.hidden,false);assert.equal(h.document.activeElement,choke);assert.equal(h.state.modal,null);
});
test('equal, beat and transient actions are searchable destinations and never activated by Return',()=>{
  for(const [name,query]of [['Apply equal slices','equal'],['Chop by beats','beats'],['Detect transients','transient']]){
    const h=harness(),button=h.n('button',{},name);button.onclick=()=>{throw Error('Action unexpectedly activated');};h.panel('Slice map',[button]);h.ui.show();h.search(query);h.key('Enter');assert.equal(h.document.activeElement,button);
  }
});
test('native root note and envelope number/range pairs appear once and prefer direct numeric entry',()=>{
  const h=harness();for(const [key,name]of [['rootNote','Root note · MIDI'],['attack','Attack · seconds']])h.panel(key==='attack'?'Amplitude envelope':'Playback region',[h.n('label',{},name,[h.n('input',{id:'range-'+key,type:'range','data-inst-param':key,'aria-label':name}),h.n('input',{id:'number-'+key,type:'number','data-inst-param':key,'data-name':name})])]);
  h.ui.show();h.search('root note');assert.equal(h.results().length,1);h.key('Enter');assert.equal(h.document.activeElement.id,'number-rootNote');h.ui.show();assert.match(h.search('amplitude attack'),/Attack/);h.key('Enter');assert.equal(h.document.activeElement.id,'number-attack');
});
test('wrapped MIDI select labels exclude option text and LFO section context distinguishes controls',()=>{
  const h=harness();h.panel('Hardware connection',[h.n('label',{},'MIDI output',[h.n('select',{id:'midi'},'',[h.n('option',{},'Secret device name')])])]);
  h.panel('Modulation',[h.n('div',{'data-mod-lfo':'lfo2'},'',[h.n('span',{class:'mod-lfo-name'},'LFO 2'),h.field('Depth','depth',{'data-mod-param':'lfo2Depth','data-mod-instrument':'0'})])]);
  h.ui.show();assert.match(h.search('midi output'),/MIDI output/);assert.doesNotMatch(h.search('secret device'),/Secret/);h.search('lfo 2 depth');h.key('Enter');assert.equal(h.document.activeElement.id,'depth');
});
test('closed detection settings remain searchable and open only on a deliberate jump',()=>{
  const h=harness(),details=h.n('details',{id:'settings'},'',[h.n('summary',{},'Detection settings'),h.field('Minimum gap · ms','gap')]);h.panel('Slice map',[details]);h.ui.show();h.search('minimum gap');assert(!details.open);h.key('Enter');assert.equal(details.open,true);assert.equal(h.document.activeElement.id,'gap');
});
test('disabled controls are labelled unavailable and hidden controls are omitted',()=>{
  const h=harness();h.panel('Slice map',[h.n('button',{disabled:'',id:'detect'},'Detect transients'),h.n('button',{hidden:'',id:'cancel'},'Cancel analysis')]);h.ui.show();assert.match(h.search('transients'),/Unavailable/);h.key('Enter');assert.equal(h.state.modal,'instrument-navigation');assert.equal(h.document.activeElement.id,'instrument-find-search');assert.match(h.notices.at(-1),/unavailable/);assert.match(h.search('cancel analysis'),/No matching/);
});
test('NKS numeric/range pairs retain page names, aliases, and separate step actions',()=>{
  const h=harness(),number=h.n('input',{id:'nks-number',type:'number','data-nks-address':'270544960','data-nks-name':'Brightness','aria-label':'Brightness normalized value'});h.panel('Preset one',[h.n('select',{'data-nks-page':''},'',[h.n('option',{selected:''},'2. Performance')]),number,h.n('input',{type:'range','data-nks-address':'270544960','aria-label':'Brightness'}),h.n('button',{'aria-label':'Use Brightness on the selected tracker step'},'Use on step…')],{'data-nks-root':'0','aria-label':'NKS mapped controls'});
  h.ui.show();h.search('brightness performance');assert.equal(h.results().length,2);h.key('Enter');assert.equal(h.document.activeElement,number);
});
test('piano keys and inactive roving slice rows do not bury section destinations',()=>{
  const h=harness();h.panel('Slice map',[h.n('div',{'data-sampler-row':'0'},'',[h.n('button',{tabindex:'0'},'Selected slice')]),h.n('div',{'data-sampler-row':'1'},'',[h.n('button',{tabindex:'-1'},'Other slice')]),h.n('button',{'data-piano':'60'},'C4')]);h.ui.show();assert.doesNotMatch(h.search('other slice'),/Other slice/);assert.doesNotMatch(h.search('c4'),/>C4</);
});
test('Escape returns exact prior focus and Arrow keys only select results',()=>{
  const h=harness(),first=h.n('button',{},'First control'),second=h.n('button',{},'Second control');h.workspace.append(first);h.workspace.append(second);h.ui.show();h.search('control');h.key('ArrowDown');assert.equal(h.get('instrument-find-search').getAttribute('aria-activedescendant'),'instrument-find-result-1');assert.equal(h.document.activeElement.id,'instrument-find-search');h.key('Escape');assert.equal(h.document.activeElement,h.invoker);assert.equal(h.state.modal,null);
});
test('replaced instruments and detached or reused NKS nodes never receive a stale jump',()=>{
  for(const replacement of ['instrument','node']){const h=harness(),old=h.n('input',{id:'nks-number',type:'number','aria-label':'Color'});h.workspace.append(old);h.ui.show();h.search('color');if(replacement==='instrument')h.state.project={instruments:[{name:'New'}]};else{old.remove();h.workspace.append(h.n('input',{id:'nks-number',type:'number','aria-label':'Different parameter'}));}h.key('Enter');assert.notEqual(h.document.activeElement,old);assert.notEqual(h.document.activeElement,h.get('nks-number'));}
});
test('Cmd-F is instrument scoped, respects dialogs/composition, and never consumes ordinary typing',()=>{
  const h=harness(),event=extra=>({key:'f',code:'KeyF',metaKey:true,preventDefault(){this.prevented=true;},stopPropagation(){},...extra});assert.equal(h.ui.key(event({isComposing:true})),false);h.state.view='mixer';assert.equal(h.ui.key(event()),false);h.state.view='instrument';h.state.modal='recording';assert.equal(h.ui.key(event()),false);h.state.modal=null;assert.equal(h.ui.key(event({metaKey:false})),false);const valid=event();assert.equal(h.ui.key(valid),true);assert(valid.prevented);assert.equal(h.ui.key(event()),true);assert(h.get('instrument-find-search').selected);
});
for(const [name,run]of tests){run();console.log('PASS '+name);}console.log(`${tests.length} instrument navigation groups passed.`);
