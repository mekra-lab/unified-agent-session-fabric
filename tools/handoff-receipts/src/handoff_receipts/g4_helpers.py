"""Decode the retained prototype RPC error envelope without logging its body."""
import json

def rpc_error_body(method, error):
    """Rpc.call wraps JSON-RPC errors as RuntimeError(method + ': ' + JSON)."""
    prefix = method + ': '
    if isinstance(error, RuntimeError) and str(error).startswith(prefix):
        try:
            body = json.loads(str(error)[len(prefix):])
            return body if isinstance(body, dict) else None
        except ValueError:
            pass
    return None
