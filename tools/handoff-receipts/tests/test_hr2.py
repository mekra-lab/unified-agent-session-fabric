"""Invented Claude records and frozen synthetic Codex 0.1.1 evidence."""
import contextlib
import io
import json
import os
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest.mock import patch
from python_fakes import enable_python_fakes

from handoff_receipts import capture, claude, query, export
from handoff_receipts.cli import main
from handoff_receipts.common import canonical, digest, private_dir, write_bytes, write_json
from handoff_receipts.store import Store, CORE

FAKE = Path(__file__).with_name('fake_claude.py').resolve()
CODEX = Path(__file__).with_name('fake_exec.py').resolve()
FIXTURE = Path(__file__).with_name('fixtures') / 'codex-011.json'


class HR2Test(unittest.TestCase):
    def setUp(self):
        enable_python_fakes(self)
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.store = Store(self.root / 'store')
        self.projects = self.root / 'projects'
        self.prompt = self.root / 'input.txt'
        self.prompt.write_text(' \nSynthetic HR2 instruction: café.\n ')
        self.env = patch.dict(os.environ, {'FAKE_CLAUDE_PROJECTS': str(self.projects),
                                          'FAKE_CLAUDE_MODE': 'completed',
                                          'FAKE_SESSIONS': str(self.root / 'codex-sessions'),
                                          'FAKE_MODE': 'completed'})
        self.env.start()

    def tearDown(self):
        self.env.stop();self.tmp.cleanup()

    def prepare(self, **kw):
        return capture.prepare(self.store, self.prompt, engine='claude', claude=FAKE,
                               cwd=self.root, session_root=self.projects,
                               observation_timeout=0, **kw)

    def run_case(self, mode='completed', **kw):
        with patch.dict(os.environ, {'FAKE_CLAUDE_MODE': mode}):
            aid = self.prepare(**kw)
            code = capture.execute(self.store, aid)
        return aid, code, query.inspect(self.store, aid)

    def test_sealed_session_exact_input_and_reported_completion(self):
        aid = self.prepare(model='synthetic-model', effort='low')
        source, request, state, _ = self.store.load(aid)
        self.assertEqual(source['engine'], 'claude')
        self.assertEqual(source['interpretation_profile'], claude.PROFILE)
        self.assertIn(request['thread_id'], request['argv'])
        self.assertIn('--session-id', request['argv'])
        self.assertNotIn('--safe-mode', request['argv'])
        self.assertNotIn('--tools', request['argv'])
        self.assertIn('--effort', request['argv'])
        self.assertEqual(state['control'][0]['event'], 'intent_recorded')
        sealed = (self.store.path(aid) / 'request.json').read_bytes()
        self.assertEqual(capture.execute(self.store, aid), 0)
        row = query.inspect(self.store, aid)
        self.assertTrue(row['facts']['target_input_confirmed'])
        self.assertEqual(row['facts']['turn_started'], 'unknown')
        self.assertEqual(row['facts']['completed_at'], 'unknown')
        self.assertEqual(row['facts']['model_read_or_used'], 'unknown')
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(row['fact_levels']['turn_status'], 'reported')
        self.assertEqual(row['fact_levels']['final_output_sha256'], 'reported')
        self.assertEqual(row['turn_key'], {'kind': 'user_message', 'value': row['turn_id']})
        self.assertEqual(row['auxiliary']['native_message_end'], 'end_turn')
        self.assertTrue(row['auxiliary']['matches_reported_output'])
        self.assertEqual(sealed, (self.store.path(aid) / 'request.json').read_bytes())

    def test_resume_prefix_same_body_and_same_engine_retry(self):
        first, _, before = self.run_case()
        second, _, after = self.run_case(resume=before['thread_id'], retry_of=first)
        self.assertEqual(after['thread_id'], before['thread_id'])
        self.assertNotEqual(after['turn_id'], before['turn_id'])
        self.assertEqual(after['group'], before['group'])
        request = self.store.load(second)[1]
        self.assertGreater(request['preflight']['bytes'], 0)
        self.assertIn('--resume', request['argv'])
        dup = query.duplicates(self.store, first)
        self.assertTrue(dup['duplicate_execution'])
        self.assertEqual(dup['effective_attempt'], 'unknown')
        path = Path(request['native_path']);path.write_bytes(b'{}\n'+path.read_bytes())
        capture.recover(self.store, second)
        self.assertIn('preflight_prefix_changed', query.inspect(self.store, second)['issues'])

    def test_multiple_assistants_and_tool_result_are_not_extra_inputs(self):
        aid, _, row = self.run_case('multiple_assistant')
        self.assertTrue(row['facts']['target_input_confirmed'])
        self.assertEqual(row['auxiliary']['native_output_sha256'], digest('CLAUDE-ACK'))
        segment = self.store.load(aid)[2]['target']['segments'][0]
        self.assertEqual(sum(i['roles'] == ['assistant'] for i in segment['lines']), 2)
        self.assertNotIn('fictional tool output', segment['copy_text'])
        self.assertEqual(len(query.which(self.store, str(self.prompt))['matches']), 1)

    def test_native_traces_do_not_promote_status_and_conflict_with_completion(self):
        for mode, expected in [('synthetic_error', 'failed'), ('interrupted', 'interrupted'),
                               ('native_error_report_completed', 'unknown'), ('native_interruption_report_completed', 'unknown')]:
            with self.subTest(mode=mode):
                output = self.root / (mode+'.md')
                _, _, row = self.run_case(mode, output=output)
                self.assertEqual(row['facts']['turn_status'], expected)
                self.assertEqual(row['fact_levels']['turn_status'], 'unknown' if expected == 'unknown' else 'reported')
                self.assertEqual(row['observed_facts']['turn_status'], 'unknown')
                self.assertTrue(row['native_traces'])
                if expected == 'unknown':
                    self.assertIn('claude_status_conflict', row['issues'])
                    self.assertEqual(row['reported']['turn_status'], 'completed')
                self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
                self.assertFalse(output.exists())
        _, _, row = self.run_case('foreign_interruption')
        self.assertEqual(row['facts']['turn_status'], 'completed')

    def test_result_loss_with_native_negative_traces_stays_unknown(self):
        for mode, kind in [('error_result_lost','synthetic_error'),('interruption_result_lost','interruption')]:
            with self.subTest(mode=mode):
                aid,_,row = self.run_case(mode,output=self.root/(mode+'.md'))
                self.assertEqual(row['facts']['turn_status'],'unknown')
                self.assertEqual(row['facts']['final_output_sha256'],'unknown')
                self.assertIn(kind,[t['kind'] for t in row['native_traces']])
                self.assertTrue(all(t['assessment']=='observed' for t in row['native_traces']))
                capture.recover(self.store,aid)
                self.assertEqual(query.inspect(self.store,aid)['facts']['turn_status'],'unknown')
                self.assertFalse((self.root/(mode+'.md')).exists())

    def test_input_message_pairs_alone_do_not_prove_claude_duplicates(self):
        first,_,before=self.run_case('result_lost')
        second,_,after=self.run_case('result_lost',retry_of=first,resume=before['thread_id'])
        self.assertNotEqual(before['turn_id'],after['turn_id'])
        self.assertTrue(before['facts']['target_input_confirmed'])
        self.assertTrue(after['facts']['target_input_confirmed'])
        self.assertEqual(query.duplicates(self.store,first)['duplicate_execution'],'unknown')

    def test_result_loss_and_recover_do_not_promote_native_completion(self):
        aid, _, row = self.run_case('result_lost', output=self.root/'lost.md')
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['auxiliary']['native_message_end'], 'end_turn')
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        capture.recover(self.store, aid)
        self.assertEqual(query.inspect(self.store, aid)['facts']['turn_status'], 'unknown')
        self.assertFalse((self.root/'lost.md').exists())

    def test_missing_native_safe_result_can_supply_output(self):
        output = self.root/'answer.md'
        aid, _, row = self.run_case('missing_native', output=output)
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertEqual(row['turn_id'], 'unknown')
        self.assertIn('rollout_not_found', row['issues'])
        self.assertEqual(output.read_text(), 'CLAUDE-ACK')
        self.assertEqual(row['output']['source'], 'claude_json_result')
        self.assertEqual(row['output']['assessment'], 'reported')
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)

    def test_sensitive_metadata_and_required_lines_are_never_copied(self):
        for mode in ('completed','sensitive_input','sensitive_assistant','sensitive_result'):
            aid, _, row = self.run_case(mode, output=self.root/(mode+'.md'))
            for path in self.store.path(aid).rglob('*'):
                if path.is_file():
                    raw = path.read_bytes()
                    for secret in [b'fictional.person@example.invalid', b'FAKE_ORG_NOT_TO_PERSIST', b'FAKE_AUTH_NOT_TO_PERSIST', b'FAKE_PRIVATE_CONTEXT']:
                        self.assertNotIn(secret, raw)
            if mode == 'sensitive_result':
                self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
                self.assertFalse((self.root/(mode+'.md')).exists())
            if mode == 'sensitive_input':
                self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
            if mode in ('sensitive_input', 'sensitive_assistant'):
                self.assertIn('sensitive_required_lines', row['issues'])

    def test_identity_mismatch_duplicate_result_and_ambiguous_input(self):
        _, _, row = self.run_case('identity_mismatch')
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertIn('claude_identity_mismatch', row['issues'])
        _, _, row = self.run_case('duplicate_result')
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        _, _, row = self.run_case('ambiguous_input')
        self.assertIn('ambiguous_target_match', row['issues'])
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')

    def test_arguments_refused_before_process_or_store_creation(self):
        options = ['--session-id', '--session-id=x', '--resume=x', '-rX', '--continue', '-c',
                   '--fork-session', '--output-format=json', '--input-format=stream-json',
                   '--print', '-p', '-pc', '--no-session-persistence', '--model=x', '-mX',
                   '--effort=low', '--session', '--', 'positional prompt', '--unknown-option',
                   '--bg', '--background', '--agent', '--agents', '--debug-file']
        with patch('handoff_receipts.capture.subprocess.Popen') as launch:
            for arg in options:
                with self.subTest(arg=arg), self.assertRaises(ValueError):self.prepare(extra=[arg])
            with self.assertRaises(ValueError):self.prepare(effort='ultra')
            with self.assertRaises(ValueError):self.prepare(via='daemon')
            launch.assert_not_called()
        self.assertEqual(self.store.ids(), [])
        aid = self.prepare(extra=['--tools', '', '--safe-mode', '--strict-mcp-config'])
        self.assertIn('--safe-mode', self.store.load(aid)[1]['argv'])

    def test_cli_exit_status_preserves_process_and_result_semantics(self):
        for mode, expected in [('completed',0),('missing_native',0),('synthetic_error',1),
                               ('interrupted',2),('result_lost',2),('identity_mismatch',2),('native_error_report_completed',2),('native_interruption_report_completed',2),('error_result_lost',2),('interruption_result_lost',2)]:
            with self.subTest(mode=mode), patch.dict(os.environ, {'FAKE_CLAUDE_MODE': mode}), contextlib.redirect_stdout(io.StringIO()) as stdout, contextlib.redirect_stderr(io.StringIO()):
                code = main(['--store',str(self.store.root),'run',str(self.prompt),'--engine','claude',
                             '--claude',str(FAKE),'-C',str(self.root),'--session-root',str(self.projects),
                             '--observation-timeout','0','--json'])
            self.assertEqual(code, expected)
            self.assertEqual(json.loads(stdout.getvalue())['engine'],'claude')

    def test_mixed_store_collision_qualified_queries_and_cross_retry_rejection(self):
        codex = capture.prepare(self.store,self.prompt,codex=CODEX,cwd=self.root,
                                session_root=self.root/'codex-sessions',observation_timeout=0)
        capture.execute(self.store,codex)
        cr = query.inspect(self.store,codex)
        with patch.dict(os.environ,{'FAKE_USER_UUID':cr['turn_id']}):
            aid,_,ar = self.run_case()
        self.assertEqual(len(query.records(self.store)),2)
        self.assertEqual(len(query.records(self.store,'claude')),1)
        with self.assertRaises(ValueError):query.lookup(self.store,cr['turn_id'])
        self.assertEqual(query.lookup(self.store,'claude:'+cr['turn_id'])[0]['attempt_id'],aid)
        self.assertEqual(query.lookup(self.store,cr['turn_id'],'codex')[0]['attempt_id'],codex)
        self.assertEqual(len(query.which(self.store,str(self.prompt))['matches']),2)
        self.assertEqual(len(query.which(self.store,str(self.prompt),'claude')['matches']),1)
        self.assertEqual(set(cr['facts']),set(ar['facts']))
        with self.assertRaises(ValueError):self.prepare(retry_of=codex)
        with self.assertRaises(ValueError):capture.prepare(self.store,self.prompt,cwd=self.root,retry_of=aid)
        export.jsonl(self.store,[codex,aid],self.root/'export.jsonl')
        rows=[json.loads(s) for s in (self.root/'export.jsonl').read_text().splitlines()]
        self.assertEqual({r['receipt']['engine'] for r in rows},{'codex','claude'})

    def test_result_is_error_and_terminal_reason_control_status_not_subtype(self):
        request = {'thread_id': 'sealed-session'}
        def summarize(error, reason, subtype='success'):
            event = {'type': 'result', 'session_id': 'sealed-session', 'is_error': error,
                     'terminal_reason': reason, 'subtype': subtype, 'result': 'SAFE'}
            controls = [{'event': name, **fields} for name, fields in claude.stream_events(event, 'sealed-session')]
            return claude.reported(request, controls)
        self.assertEqual(summarize(True, 'api_error')['turn_status'], 'failed')
        self.assertEqual(summarize(False, 'completed', 'error_during_execution')['turn_status'], 'completed')
        for error, reason in [(None,'completed'), ('false','completed'), (False,None), (False,'api_error')]:
            self.assertEqual(summarize(error, reason)['turn_status'], 'unknown')
        self.assertEqual(summarize(True, 'aborted_streaming')['turn_status'], 'interrupted')

    def test_native_identity_summaries_cannot_override_retained_input(self):
        for field in ('thread_id','turn_id'):
            aid,_,_=self.run_case()
            state=self.store.load(aid)[2]
            state['target'][field]='fabricated-key'
            self.store.save(aid,state)
            with self.assertRaises(ValueError):query.inspect(self.store,aid)

    def test_semantic_inventory_tampering_and_output_recovery_override(self):
        aid,_,row = self.run_case(output=self.root/'original.md')
        request = (self.store.path(aid)/'request.json').read_bytes()
        destination = self.root/'recover.md'
        capture.recover(self.store,aid,output=destination)
        self.assertEqual(destination.read_text(),'CLAUDE-ACK')
        self.assertEqual((self.store.path(aid)/'request.json').read_bytes(),request)
        state = self.store.load(aid)[2]
        state['target']['segments'][0]['lines'][0]['roles'] = ['assistant']
        self.store.save(aid,state)
        with self.assertRaises(ValueError):query.inspect(self.store,aid)

    def test_missing_resume_boundary_never_attaches_later_native_rows(self):
        aid,_,row = self.run_case(resume=str(uuid.uuid4()))
        self.assertEqual(row['facts']['turn_status'],'completed')
        self.assertEqual(row['facts']['target_input_confirmed'],'unknown')
        self.assertIn('claude_preflight_unavailable',row['issues'])
        capture.recover(self.store,aid)
        self.assertEqual(query.inspect(self.store,aid)['facts']['target_input_confirmed'],'unknown')

    def test_missing_prompt_id_uses_proven_parent_chain(self):
        _,_,row = self.run_case('parent_only')
        self.assertTrue(row['facts']['target_input_confirmed'])
        self.assertEqual(row['auxiliary']['native_output_sha256'],digest('CLAUDE-ACK'))
        self.assertEqual(row['facts']['turn_status'],'completed')

    def test_parent_projection_inspect_and_verify_without_native_access(self):
        import shutil
        aid, _, row = self.run_case('parent_only')
        segment = self.store.load(aid)[2]['target']['segments'][0]
        self.assertTrue(segment['connection_projections'])
        shutil.rmtree(self.projects)
        with patch('handoff_receipts.claude.read_rows', side_effect=AssertionError('native read during query')):
            self.assertEqual(query.inspect(self.store, aid), row)
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(main(['--store', str(self.store.root), 'verify', aid, '--json']), 0)

    def test_native_path_is_exact_project_only(self):
        aid = self.prepare()
        request = self.store.load(aid)[1]
        other = self.projects/'other-project';other.mkdir(parents=True)
        (other/(request['thread_id']+'.jsonl')).write_text('UNRELATED_PRIVATE_RECORD')
        with patch('handoff_receipts.capture.session_paths',side_effect=AssertionError('cross-project scan')):
            capture.execute(self.store,aid)
            capture.recover(self.store,aid)
        self.assertTrue(query.inspect(self.store,aid)['facts']['target_input_confirmed'])
        with self.assertRaises(ValueError):self.prepare(resume='not-a-uuid')

    def test_offline_queries_integrity_and_legacy_bytes_unchanged(self):
        fixture=json.loads(FIXTURE.read_text());aid=fixture['attempt'];folder=self.store.path(aid)
        private_dir(folder/'states')
        for name in ('source','request'):write_json(folder/(name+'.json'),fixture[name])
        write_bytes(folder/'body.txt',fixture['body'].encode())
        write_bytes(folder/'INTERPRETATION.md',fixture['interpretation'].encode())
        write_json(folder/'manifest.json',{'version':1,'files':{name:digest((folder/name).read_bytes()) for name in CORE}})
        Store._save(folder,fixture['state'])
        before={str(p):p.read_bytes() for p in folder.rglob('*') if p.is_file()}
        with patch('handoff_receipts.claude.read_snapshot',side_effect=AssertionError('native read')):
            row=query.inspect(self.store,aid)
            self.assertEqual(row['engine'],'codex')
            del row['engine'];del row['interpretation_profile']
            self.assertEqual(row,fixture['expected'])
            query.records(self.store);query.duplicates(self.store,aid);query.which(self.store,fixture['source']['path'])
        self.assertEqual(before,{str(p):p.read_bytes() for p in folder.rglob('*') if p.is_file()})
        fresh,_,_=self.run_case()
        target=self.store.path(fresh)/'body.txt';target.write_text('tampered')
        with self.assertRaises(ValueError):query.inspect(self.store,fresh)



class AbbreviationTest(unittest.TestCase):
    def test_abbreviated_options_are_rejected(self):
        for args in (['run', 'x', '--engine', 'claude', '--sock', '/tmp/x'],
                     ['run', 'x', '--engine', 'claude', '--cod', 'codex'],
                     ['--sto', '/tmp/x', 'list']):
            with self.subTest(args=args), contextlib.redirect_stderr(io.StringIO()), \
                    self.assertRaises(SystemExit) as raised:
                main(args)
            self.assertEqual(raised.exception.code, 2)


if __name__ == '__main__':
    unittest.main()
