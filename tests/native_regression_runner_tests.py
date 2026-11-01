#!/usr/bin/env python3
"""Pure Python tests for native runner evidence/timeout handling; launches no native fixture."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))
from native_regression_support import checked_relative,digest,execute,private_environment,run_modes,validate_report,verify_build,write_json

class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary=tempfile.TemporaryDirectory();self.folder=Path(self.temporary.name)
    def tearDown(self):
        self.temporary.cleanup()
    def test_report_requires_literal_true_and_all_nonempty_checks(self):
        report=self.folder/'report.json'
        for value in [[],{}, {'passed':True,'checks':[]},{'passed':1,'checks':[{'passed':True}]},{'passed':True,'checks':[{'passed':False}]},{'passed':True,'checks':[{'passed':1}]}]:
            write_json(report,value)
            with self.assertRaises(ValueError):validate_report(report)
        write_json(report,{'passed':True,'checks':[{'passed':True}]});self.assertEqual(len(validate_report(report)['checks']),1)
    def test_missing_or_malformed_reports_fail(self):
        report=self.folder/'report.json'
        with self.assertRaises(ValueError):validate_report(report)
        report.write_text('{')
        with self.assertRaises(ValueError):validate_report(report)
    def test_private_environment_preserves_home_and_isolates_overrides(self):
        old=dict(os.environ)
        try:
            os.environ['SUPERTRACKER_SUPPORT_DIR']='/must-not-be-used';os.environ['ST_UNSAFE_OVERRIDE']='bad';os.environ['DYLD_INSERT_LIBRARIES']='bad'
            result=private_environment(self.folder)
            self.assertEqual(result.get('HOME'),old.get('HOME'));self.assertNotIn('ST_UNSAFE_OVERRIDE',result);self.assertNotIn('DYLD_INSERT_LIBRARIES',result)
            self.assertEqual(result['SUPERTRACKER_SUPPORT_DIR'],str(self.folder/'support'));self.assertTrue((self.folder/'tmp').is_dir())
        finally:os.environ.clear();os.environ.update(old)
    def test_timeout_terminates_and_records_process(self):
        result=execute([sys.executable,'-c','import time;print("started",flush=True);time.sleep(30)'],self.folder,'timeout',.1)
        self.assertTrue(result['timedOut']);self.assertNotEqual(result['exitCode'],0);self.assertLess(result['seconds'],5)
        self.assertIn('started',(self.folder/'timeout.stdout').read_text());self.assertTrue((self.folder/'timeout-status.json').is_file())
    def fixture(self):
        (self.folder/'source').mkdir();(self.folder/'source/input.txt').write_text('pinned source')
        executable=self.folder/'pure-python-fixture'
        executable.write_text('#!'+sys.executable+'\nimport json,pathlib,sys\nmode=sys.argv[1];out=pathlib.Path(sys.argv[-1]);report={"passed":True,"checks":[{"passed":True}]}\nif mode=="bad-report":report["checks"][0]["passed"]=False\nif mode!="missing":(out/"report.json").write_text(json.dumps(report))\nsys.exit(7 if mode=="exit-error" else 0)\n')
        executable.chmod(0o755)
        manifest=self.folder/'source-manifest.json';write_json(manifest,{'input.txt':digest(self.folder/'source/input.txt')})
        write_json(self.folder/'build.json',{'schema':1,'fixture':'pure-python','built':True,'executable':executable.name,'sourceManifestSHA256':digest(manifest),'artifacts':{executable.name:digest(executable)}})
        return executable
    def test_sequential_modes_require_process_and_report_success(self):
        self.fixture();modes=[(mode,[mode]) for mode in ['good','bad-report','exit-error','missing']]
        self.assertFalse(run_modes(self.folder,'pure-python',modes,5))
        summary=json.loads(next((self.folder/'runs').glob('*/results.json')).read_text())
        self.assertTrue(summary['complete']);self.assertEqual([r['passed'] for r in summary['results']],[True,False,False,False])
        self.assertEqual([r['mode'] for r in summary['results']],['good','bad-report','exit-error','missing'])
    def test_modified_artifact_prevents_execution(self):
        executable=self.fixture();executable.write_text('changed')
        with self.assertRaisesRegex(ValueError,'artifact changed'):verify_build(self.folder,'pure-python')
        self.assertFalse((self.folder/'runs').exists())
    def test_modified_source_or_manifest_prevents_execution(self):
        self.fixture();(self.folder/'source/input.txt').write_text('changed')
        with self.assertRaisesRegex(ValueError,'Source changed'):verify_build(self.folder,'pure-python')
        (self.folder/'source-manifest.json').write_text('{}')
        with self.assertRaisesRegex(ValueError,'manifest changed'):verify_build(self.folder,'pure-python')
    def test_manifest_cannot_escape_private_directory(self):
        with self.assertRaises(ValueError):checked_relative(self.folder,'../outside')
        with self.assertRaises(ValueError):checked_relative(self.folder,'/tmp/outside')
        self.assertEqual(checked_relative(self.folder,'safe'),(self.folder/'safe').resolve())

if __name__=='__main__':unittest.main()
