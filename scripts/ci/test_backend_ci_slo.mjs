import assert from "node:assert/strict";
import { SLO_MS, formatDuration, percentile, summarizeBackendJobs } from "./backend_ci_slo.mjs";

const at = (minutes) => `2026-08-31T00:${String(minutes).padStart(2, "0")}:00Z`;
const job = (start, end) => ({ name: "test-backend", started_at: at(start), completed_at: at(end) });
assert.equal(formatDuration(SLO_MS), "15m 0s");
assert.equal(percentile([4, 1, 3, 2], 0.5), 2);
assert.equal(percentile([4, 1, 3, 2], 0.95), 4);
const summary = summarizeBackendJobs([job(0, 14), job(15, 31), job(32, 49), { name: "test-frontend", started_at: at(0), completed_at: at(59) }, { name: "test-backend", started_at: "invalid", completed_at: at(1) }]);
assert.equal(summary.samples.length, 3);
assert.equal(summary.medianMs, 16 * 60 * 1000);
assert.equal(summary.p95Ms, 17 * 60 * 1000);
assert.equal(summary.consecutiveBreaches, 2);
