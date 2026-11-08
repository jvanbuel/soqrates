# evals

Eval harness and benchmarks for Soqrates (ADR-0001 D13), built on
[Inspect AI](https://inspect.aisi.org.uk/). Evals drive the `soqrates` binary
or MCP server as a black box.

Evals call real models, so they need API keys and cost money. They are
Python, outside the Cargo workspace, and never run as part of `cargo test`;
the hermetic tests stay in the crates (ADR-0001 D12).

## Running

```sh
cargo build --release -p soqrates-cli
cd evals
export SOQRATES_BIN=../target/release/soqrates
export ANTHROPIC_API_KEY=...          # for soqrates and for the judge
export SOQRATES_MODEL=anthropic:claude-sonnet-5-5   # optional: the examiner
uv run inspect eval soqrates_evals/option_consistency.py@option_consistency \
    --model anthropic/claude-sonnet-5-5           # the judge
uv run inspect view                                # browse the logs
```

Each soqrates run uses a fresh state directory and config directory, so the
author is always a new learner (a novice, who gets multiple-choice
questions) and your own configuration is never read.

## Evals

### Explanation-option consistency (issue #5)

`soqrates_evals/option_consistency.py`

- `option_consistency` runs a session on each diff in
  `soqrates_evals/data/diffs/`, answers every multiple-choice question
  (option `pick`, default 0, so a wrong pick's reason is judged along with
  the explanation), and has an LLM judge decide whether the feedback argues
  about an answer the question did not offer. The score is the share of
  questions judged consistent (a diff that gave no multiple-choice question
  scores 0). Pass `-T judge_model=...` to judge with a model other than
  `--model`, and `--epochs` for more questions per diff.
- `option_consistency_judge` checks the judge against the hand-labelled
  cases in `soqrates_evals/data/labels.jsonl`. Run it whenever the judge
  prompt or model changes, and add spot-checked real outputs to the labels
  (flagged and unflagged ones from `inspect view`), with the source noted.

The examiner itself runs a cheaper check of the same kind on every question
(ADR-0004 D1); this eval measures how much still gets through.

## Planned metrics

- relevance of questions to the diff
- how well difficulty matches the author's level
- how often the grader agrees with human labels
- simulated retention over repeated sessions
