# evals

Eval harness and benchmarks for Soqrates (ADR-0001 D13), built on
[Inspect AI](https://inspect.aisi.org.uk/). Evals drive the `soqrates` binary
or MCP server as a black box.

Planned metrics:

- relevance of questions to the diff
- how well difficulty matches the author's level
- how often the grader agrees with human labels
- simulated retention over repeated sessions

Not started: this needs a working binary first.
