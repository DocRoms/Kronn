# HTTP tool refusal convergence

An HTTP model can ignore a withdrawn tool and keep varying its arguments, so
exact-call repetition is not sufficient to stop a refusal loop. Kronn classifies
only its own non-executed anti-loop outcomes as loop refusals; executor failures
and worker-stage policy refusals keep their separate handling.
[src: file: backend/src/agents/runner.rs:8222-8239#is_loop_refusal]

The runner bounds this behavior at two levels. Within one provider response it
answers at most eight loop refusals, then omits only later calls that would also
be refused. Across responses, a discussion or audit is asked to synthesize
after two refusal-only rounds; a worker keeps its tools. Three refusal-only
rounds in one phase stop a worker or audit as a failure. A discussion never
reaches that stop: synthesis removes its tools and a stray call ends on the
tool-free fallback. An audit's write phase and each new worker phase count from
zero, so convergence never takes the rounds an audit needs to write. Room-visible
refusal traces are limited to two per logical tool and round, while aggregate
refusal counts remain available to the ceiling report and final diagnostic.
[src: file: backend/src/agents/runner.rs:50-55#MAX_LOOP_REFUSALS_PER_RESPONSE]
[src: file: backend/src/agents/runner.rs:8244-8274#RefusalTraces]
[src: file: backend/src/agents/runner.rs:12052-12078]
[src: file: backend/src/agents/runner.rs:12219-12222]

History balance is an invariant: dropped calls are removed from the assistant
tool-call message as well as from tool results, so every declared call id still
has exactly one answer. A valid unrelated call later in the same provider
response remains executable; dropping is decided per call, not for the rest of
the batch wholesale.
[src: file: backend/src/agents/runner.rs:10855-10909]
[src: file: backend/src/agents/runner.rs:11567-11597]
[src: file: backend/src/agents/runner.rs:11909-11914]
[src: file: backend/src/agents/runner_test.rs:14435-14501#a_huge_same_response_batch_past_a_ceiling_is_bounded_and_balanced]
[src: file: backend/src/agents/runner_test.rs:14558-14598#a_useful_tool_after_a_refused_flood_in_one_response_still_runs]
[src: file: backend/src/agents/runner_test.rs:14603-14641#a_worker_asking_only_for_refused_tools_is_stopped_on_the_third_round]
[src: file: backend/src/agents/runner_test.rs:14681-14706#an_audit_still_writes_its_deliverable_after_refusal_convergence]
[src: file: backend/src/agents/runner_test.rs:14711-14733#an_audit_write_phase_of_refusals_only_is_stopped_on_its_third_round]
