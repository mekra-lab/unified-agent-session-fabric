"""Fixed public observation diagnostics; never interpolate native content."""
MESSAGES = {
    'agy_version_unverified': 'Antigravity version or executable hash is unverified; native interpretation is auxiliary only.',
    'agy_identity_mismatch': 'Antigravity stream conversation IDs conflict with one another or the requested conversation.',
    'agy_identity_unavailable': 'No conversation ID was preserved from this child; no native DB is searched.',
    'agy_input_index_unavailable': 'No unique user_input step_index was preserved from this child; native attribution is unknown.',
    'agy_native_identity_mismatch': 'The selected DB cascade ID differs from the stream/requested conversation.',
    'agy_boundary_changed': 'The selected DB trajectory or pre-dispatch boundary no longer matches.',
    'agy_native_busy': 'The selected native DB is busy; bounded rereads may observe a later commit.',
    'agy_native_incomplete': 'The selected native DB is not readable or complete yet.',
    'agy_executor_pending': 'No readable same-execution executor metadata is available yet.',
    'agy_fields_omitted': 'Some auxiliary native fields could not be safely interpreted or retained.',
    'agy_result_unavailable': 'No unique, complete Antigravity result is available; native completion is not inferred.',
    'agy_status_conflict': 'Antigravity reported completion conflicts with USER_CANCELED for the same execution.',
    'claude_status_conflict': 'Claude reported completion conflicts with a native error or interruption trace; the outcome is unknown.',
    'claude_preflight_unavailable': 'The selected Claude project had no preflight transcript; native attribution remains unknown.',
    'claude_identity_mismatch': 'The Claude stream session ID differs from the sealed session ID.',
    'claude_result_unavailable': 'No unique, complete Claude result is available; native completion is not inferred.',
    'rollout_not_found': 'The target rollout path or file is not available yet.',
    'rollout_unreadable': 'The target rollout could not be read.',
    'rollout_unsafe_path': 'The rollout path is not a regular file without symlinks.',
    'rollout_too_large': 'The rollout exceeds the supported 32 MiB snapshot size.',
    'rollout_incomplete': 'No complete native JSONL record is available yet.',
    'rollout_invalid_json': 'A complete native line is not valid JSON.',
    'rollout_invalid_shape': 'A native record has an unsupported shape.',
    'rollout_metadata_missing': 'The first complete record is not session metadata.',
    'rollout_thread_mismatch': 'The native session ID differs from the requested thread.',
    'preflight_prefix_changed': 'The saved preflight byte prefix no longer matches.',
    'ambiguous_rollout_path': 'More than one rollout path matches the thread.',
    'no_matching_turn': 'No eligible turn contains the exact input after the preflight boundary.',
    'ambiguous_target_match': 'More than one eligible native turn matches the request.',
    'input_binding_omitted': 'Sensitive required lines prevent binding the input to this turn.',
    'sensitive_required_lines': 'Required native lines were omitted by the privacy check.',
    'target_input_missing': 'The selected turn does not retain an exact input confirmation.',
    'turn_terminal_missing': 'The selected native turn has no retained terminal event yet.',
    'final_output_missing': 'The completed native turn has no retained final output.',
    'native_selection_invalid': 'The native selection could not be safely interpreted.',
    'target_observation_unavailable': 'Legacy capture found no target observation; no specific reason was retained.',
    'legacy_observation_error': 'Legacy capture retained only a generic observation error.',
}
RETRYABLE = {'rollout_not_found', 'rollout_incomplete', 'no_matching_turn',
             'agy_native_busy', 'agy_native_incomplete', 'agy_executor_pending',
             'target_input_missing', 'turn_terminal_missing', 'final_output_missing'}


def issue(code, **details):
    return {'code': code, 'description': MESSAGES[code], **({'details': details} if details else {})}


class ObservationError(ValueError):
    def __init__(self, code, **details):
        self.diagnostic = issue(code, **details)
        super().__init__(MESSAGES[code])


def stored_issues(target):
    if 'diagnostics' in target:
        return target['diagnostics']
    return [issue(code if code in MESSAGES else 'legacy_observation_error') for code in target.get('issues', [])]
