# Subagent cost evidence

Claude Code's `cost.total_cost_usd` already includes subagent (Task tool)
spend. tachobar shows that figure as is. Adding the subagent transcripts on
top counted the same spend twice.

## Measurement

| Item | Value |
| --- | --- |
| Session id | `6e1a7121-8400-4a30-b1cf-7a14984336a8` |
| Claude Code version | v2.1.294 |
| Method | `claude -p --output-format json`, model `claude-haiku-5-5`, one Agent-tool subagent |
| `total_cost_usd` | 0.009757905 |
| Main conversation (priced) | 0.006449 |
| Subagent (priced) | 0.003309 |
| Main + subagent | 0.009758 |
| total - (main + subagent) | 0 |
| total - main | 0.003309 (equals the subagent) |

Token counts from `result.json` `modelUsage`, each equal to main + subagent:

| Bucket | `modelUsage` | Main | Subagent | Main + subagent |
| --- | --- | --- | --- | --- |
| input | 8 | 4 | 4 | 8 |
| output | 522 | 290 | 232 | 522 |
| cache read | 49893 | 29447 | 20446 | 49893 |
| cache write | 53951 | 30044 | 23907 | 53951 |

## Reading the numbers

- `total_cost_usd` equals main + subagent to the last digit, so it includes the
  subagent.
- Before this change tachobar computed `total_cost_usd` + subagent USD, which
  for this session is 0.009758 + 0.003309 = 0.013067 instead of 0.009758.
- Token equality does not depend on prices. The USD split was computed with
  haiku-4-5 list rates scaled by 0.1, because `claude-haiku-5-5` has no entry
  in the price table. Only the split is affected by that choice; the total is
  Claude Code's own figure.

## Caveat

The figure is the `-p` result total, taken from the same cost state as the
transcript's `totalCostUSD`. It is not a captured interactive status-line
payload.

## Fixture

`tests/fixtures/subagent-cost/` holds this session, anonymised to usage fields
only (main transcript, one subagent transcript, and the status-line stdin).
Its token sums match the table above, and `cli.rs` checks that the rendered
cost matches the stdin total.
