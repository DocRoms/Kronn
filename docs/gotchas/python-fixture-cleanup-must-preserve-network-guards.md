# Python fixture cleanup must preserve network guards

The MCP bridge tests install a module-wide `urllib.request.urlopen` refusal
guard. Each case replaces only the transport it exercises; module teardown
checks that the guard still exists and recorded no ambient calls.

A publication-grant fixture used `mock.patch.stopall()` in its cleanup. That
also stopped the module guard, so the merged suite finished with 809 test cases
but a teardown error. A passing case did not establish isolation for the next
case. Do not remove the teardown assertion or accept that run as qualification.

Keep each patcher and register only its own `stop` method with `addCleanup`.
The grant fixture now additionally checks guard identity after its own cleanups.
This is fixture ownership, not a change to the bridge's publication policy.
[src: file: backend/scripts/test_disc_introspection_mcp.py:54]
[src: file: backend/scripts/test_disc_introspection_mcp.py:417]
