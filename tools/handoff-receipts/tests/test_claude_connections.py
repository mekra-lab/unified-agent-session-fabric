"""Synthetic parent chains and offline derived projections, with no native fixtures."""
import copy
import json
import unittest
from handoff_receipts import claude
from handoff_receipts.common import digest


class ConnectionTest(unittest.TestCase):
    def select(self, middle=(), parent='input', before=()):
        rows = list(before) + [{'type': 'user', 'uuid': 'input', 'parentUuid': None,
            'message': {'role': 'user', 'content': 'instruction'}}] + list(middle) + [
            {'type': 'assistant', 'uuid': 'answer', 'parentUuid': parent,
             'message': {'role': 'assistant', 'content': [{'type': 'text', 'text': 'answer'}], 'stop_reason': 'end_turn'}}]
        raw = b'';parsed = []
        for row in rows:
            row = {'sessionId': 'synthetic-session', **row}
            line = (json.dumps(row)+'\n').encode()
            parsed.append((len(raw), len(raw)+len(line), row));raw += line
        boundary = parsed[len(before)][0]
        request = {'thread_id': 'synthetic-session', 'preflight': {'bytes': boundary, 'sha256': digest(raw[:boundary])}}
        segment = claude.select(raw, parsed, request, b'instruction')
        return segment

    def observed(self, segment):
        return claude.observation(segment, b'instruction', 'synthetic-session')

    def test_direct_parent_chain(self):
        segment = self.select()
        self.assertEqual(self.observed(segment)['auxiliary']['native_output_sha256'], digest('answer'))
        self.assertEqual(segment['connection_projections'], [])

    def test_attachment_bridge_has_only_derived_ids_and_provenance(self):
        segment = self.select([{'type': 'attachment', 'uuid': 'bridge', 'parentUuid': 'input',
                               'attachment': {'userEmail': 'invented@example.invalid', 'payload': 'PRIVATE_SENTINEL'}}], 'bridge')
        self.assertEqual(self.observed(segment)['auxiliary']['native_output_sha256'], digest('answer'))
        projection, = segment['connection_projections']
        self.assertEqual(projection['fields'], {'uuid': 'bridge', 'parentUuid': 'input'})
        self.assertEqual(set(projection), {'kind', 'fields', 'start', 'end', 'sha256'})
        self.assertNotIn('PRIVATE_SENTINEL', json.dumps(segment))
        self.assertNotIn('invented@example.invalid', json.dumps(segment))
        altered = copy.deepcopy(segment);altered['connection_projections'][0]['fields']['type'] = 'attachment'
        with self.assertRaises(ValueError):self.observed(altered)
        altered = copy.deepcopy(segment);altered['connection_projections'][0]['sha256'] = '0'*64
        with self.assertRaises(ValueError):self.observed(altered)
        altered = copy.deepcopy(segment);altered['connection_projections'][0]['fields']['parentUuid'] = 'missing'
        with self.assertRaises(ValueError):self.observed(altered)

    def test_other_real_input_is_a_barrier(self):
        segment = self.select([{'type': 'user', 'uuid': 'other', 'parentUuid': 'input',
                               'message': {'role': 'user', 'content': 'another instruction'}}], 'other')
        self.assertEqual(self.observed(segment)['auxiliary']['native_output_sha256'], 'unknown')
        self.assertEqual(segment['connection_projections'], [])

    def test_broken_and_outside_append_chains(self):
        for before in ([], [{'type': 'attachment', 'uuid': 'outside', 'parentUuid': 'input'}]):
            segment = self.select(parent='outside', before=before)
            self.assertEqual(self.observed(segment)['auxiliary']['native_output_sha256'], 'unknown')

    def test_cycle_is_unknown(self):
        segment = self.select([{'type': 'attachment', 'uuid': 'cycle', 'parentUuid': 'answer'}], 'cycle')
        self.assertEqual(self.observed(segment)['auxiliary']['native_output_sha256'], 'unknown')

    def test_malformed_unneeded_id_does_not_discard_input(self):
        segment = self.select([{'type': 'attachment', 'uuid': {'invalid': True}, 'parentUuid': None}])
        self.assertTrue(self.observed(segment)['matches'])
        self.assertEqual(self.observed(segment)['auxiliary']['native_output_sha256'], digest('answer'))

    def test_duplicate_id_is_unknown(self):
        segment = self.select([{'type': 'attachment', 'uuid': 'duplicate', 'parentUuid': 'input'},
                               {'type': 'attachment', 'uuid': 'duplicate', 'parentUuid': 'input'}], 'duplicate')
        self.assertEqual(self.observed(segment)['auxiliary']['native_output_sha256'], 'unknown')

    def test_old_observation_keeps_explicit_prompt_rule(self):
        segment = self.select();segment.pop('association');segment.pop('connection_projections')
        # Legacy snapshots cannot silently acquire parent-derived evidence.
        with self.assertRaises(ValueError):self.observed(segment)
