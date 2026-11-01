'use strict';
// Source-boundary contracts only. These do not instantiate AppKit or run scanners.
const fs=require('node:fs'),assert=require('node:assert/strict');
const read=p=>fs.readFileSync(p,'utf8'),bridge=read('Sources/SuperTracker/Bridge.swift');
const tests=[];const test=(name,fn)=>tests.push([name,fn]);
test('normal Bridge discovery remains enabled unless explicitly disabled',()=>{
 assert.match(bridge,/pluginDiscoveryEnabled: Bool = true/);assert.match(bridge,/self\.pluginDiscoveryEnabled = pluginDiscoveryEnabled/);
});
test('ready cannot schedule the global discovery job in an opted-out fixture',()=>{
 const ready=bridge.slice(bridge.indexOf('case "ready":'),bridge.indexOf('case "engine.project":',bridge.indexOf('case "ready":')));
 assert.match(ready,/if pluginDiscoveryEnabled && keyboardAcceptance == nil \{[\s\S]*?Task[\s\S]*?startPluginScan\(\)/);
});
test('manual discovery fails before any scan and direct scan entry guards shared access',()=>{
 const manual=bridge.slice(bridge.indexOf('case "plugins.scan":'),bridge.indexOf('case "mk3.connect"'));
 assert(manual.indexOf('guard pluginDiscoveryEnabled else')>=0);assert(manual.indexOf('guard pluginDiscoveryEnabled else')<manual.indexOf('startPluginScan()'));assert.match(manual,/throw TrackerError\.message/);
 const start=bridge.slice(bridge.indexOf('func startPluginScan()'),bridge.indexOf('private func saveRecovery'));
 assert.match(start,/func startPluginScan\(\) \{\s*guard pluginDiscoveryEnabled, !pluginScanRunning else \{ return \}/);
 assert(start.indexOf('guard pluginDiscoveryEnabled')<start.indexOf('VST3Catalogue.shared'));assert(start.indexOf('guard pluginDiscoveryEnabled')<start.indexOf('VST2Catalogue.shared'));
});
test('opted-out shutdown does not instantiate the global catalogue singletons',()=>{
 const shutdown=bridge.slice(bridge.indexOf('func shutdownAndWait()'));
 assert.match(shutdown,/if pluginDiscoveryEnabled && keyboardAcceptance == nil \{ VST3Catalogue\.shared\.cancel\(\); VST2Catalogue\.shared\.cancel\(\) \}/);
});
test('all four private bridge helper hosts inject both empty-root private catalogues',()=>{
 for(const name of ['NativeUIBridgeSelfTest','NativeUIRecoveryAcceptanceTests','NativeBridgeSaveSafetyTests','NativeTerminationSaveTests']){
  const source=read('Sources/SuperTracker/'+name+'.swift');
  assert.match(source,/vst3Configuration\.roots = \[\]; vst3Configuration\.cacheURL = (?:scratch|directory)\.appendingPathComponent/,name);
  assert.match(source,/vst2Configuration\.roots = \[\]; vst2Configuration\.cacheURL = (?:scratch|directory)\.appendingPathComponent/,name);
  assert.match(source,/vst3Catalogue: VST3Catalogue\(configuration:vst3Configuration\)/,name);assert.match(source,/vst2Catalogue: VST2Catalogue\(configuration:vst2Configuration\)/,name);
  assert.match(source,/Bridge\(audio:[^\n]*pluginDiscoveryEnabled:false/,name);
 }
});
test('installed NKS diagnostic keeps its exact explicit factory while disabling general discovery',()=>{
 const source=read('tests/NativeNKSUIAcceptance.swift');assert.match(source,/catalogueConfiguration\.roots = \[\]/);assert.match(source,/catalogueConfiguration\.cacheURL = scratch\.appendingPathComponent/);assert.match(source,/catalogue\.scanBundle\(bundleURL\)/);assert.match(source,/vst3Catalogue: catalogue/);
 assert.match(source,/vst2Configuration\.roots = \[\]; vst2Configuration\.cacheURL = scratch\.appendingPathComponent/);assert.match(source,/vst2Catalogue: VST2Catalogue\(configuration:vst2Configuration\)/);assert.match(source,/Bridge\(audio:[^\n]*pluginDiscoveryEnabled:false/);
});
test('mounted bridge fixture includes an actual manual-discovery refusal assertion',()=>{
 const source=read('Sources/SuperTracker/NativeUIBridgeSelfTest.swift');assert.match(source,/bridge\.handle\("plugins.scan", payload:\[:\]\)/);assert.match(source,/error\.localizedDescription\.contains\("discovery is disabled"\)/);
});
for(const[name,fn]of tests){fn();console.log('PASS '+name);}console.log('Bridge discovery isolation: '+tests.length+' source-contract groups passed; no native execution.');
