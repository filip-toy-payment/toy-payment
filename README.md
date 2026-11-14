# toy-payment

A toy payments engine. It reads transactions from a CSV file, applies them to client accounts and prints the final state of every account as CSV. It handles deposits, withdrawals, disputes, resolves and chargebacks.

## Usage

```
cargo run -- transactions.csv > accounts.csv
```

The input file is the only argument. The accounts go to standard output. Skipped rows and errors go to standard error.

Input columns are `type`, `client`, `tx` and `amount`:

```
type, client, tx, amount
deposit, 1, 1, 1.0
deposit, 2, 2, 2.0
deposit, 1, 3, 2.0
withdrawal, 1, 4, 1.5
withdrawal, 2, 5, 3.0
```

Output for that input:

```
client,available,held,total,locked
1,1.5,0,1.5,false
2,2,0,2,false
```

| Exit code | Meaning |
|---|---|
| 0 | The whole input was processed and the accounts were written. |
| 1 | Reading the input failed. Or writing the output or the log failed. No accounts are printed. |
| 2 | Wrong invocation. Exactly one argument is required. |

Run the tests with `cargo test`. Run the lints with `cargo clippy --all-targets`.

## Structure

The crate is a library with a thin binary on top.

| File | Responsibility |
|---|---|
| `src/amount.rs` | `Amount` and `Balance`. Exact decimal money types and the cap. |
| `src/transaction.rs` | `Transaction`, `Kind`, `ClientId`, `TxId`. The input of the engine. |
| `src/account.rs` | `Account` and its five operations. |
| `src/engine.rs` | `Engine`. The business rules and the store of applied transactions. |
| `src/input.rs` | Reads a CSV stream and turns rows into transactions. |
| `src/output.rs` | Writes the accounts as CSV. |
| `src/lib.rs` | `run`. Connects input, engine and output. |
| `src/main.rs` | The argument, the file and the exit code. |

Business logic is separate from parsing and printing. The engine, the account and the money types import nothing from `csv` or `serde`. They do no I/O. The engine receives `Transaction` values and returns a typed `Rejection` when a transaction cannot apply. Everything specific to CSV stays in `input.rs` and `output.rs`.

```
main.rs    argument, file, exit code
   |
run(input, output, diagnostics)
   |
   |-- input    CSV stream           ->  Transaction or RowError
   |-- engine   apply(Transaction)   ->  Ok or Rejection
   `-- output   accounts             ->  CSV
```

## Assumptions

The specification leaves several cases open. Each one is decided below. The guiding idea is to act as a bank would.

### Disputes

| Case | Behavior | Reason |
|---|---|---|
| Which transactions can be disputed | Only deposits. A dispute that names a withdrawal is ignored. | The specified effect of a dispute describes a deposit. It lowers available funds and raises held funds. |
| Dispute for more than the available funds | The full deposit amount is held. Available funds go negative. | This is the fraud case from the specification: deposit, withdraw, then reverse the deposit. The negative balance records what the client owes. |
| Dispute, resolve or chargeback that names another client's deposit | Ignored. | No client may move another client's funds. |
| Dispute on a deposit already under dispute | Ignored. | The funds are held already. |
| Dispute on a deposit whose dispute was resolved | Allowed. | A resolve returns the deposit to its normal state. |
| Dispute on a deposit that was charged back | Ignored. | A chargeback is final. |
| An amount on a dispute, resolve or chargeback row | Not examined. The row is processed. | These rows need no amount. The amount always comes from the original deposit. |

### Locked accounts

A chargeback locks the account. A locked account rejects deposits and withdrawals. It still accepts disputes, resolves and chargebacks on earlier deposits. Those come from the payment partner and describe events that already happened. Ignoring them would leave the books wrong.

### Client accounts

Every transaction that reaches the engine creates the account of the client it names. This includes a withdrawal that fails and a dispute that is ignored. Such a client appears in the output with zero balances.

A row that cannot be parsed into a transaction creates no account.

### Amounts

| Case | Behavior |
|---|---|
| Zero or negative amount | The row is rejected. A negative deposit would be a withdrawal that skips the funds check. |
| More than four decimal places | Accepted only if the extra digits are zeros. `1.50000` is `1.5`. `1.23456` is rejected. The engine never rounds or truncates an amount. |
| Accepted number forms | Plain decimals as read by the exact parser of `rust_decimal`. This includes `+1.5`, `.5`, `5.` and `1_000.5`. Scientific notation such as `1e3` is rejected. |
| Largest amount and balance | 7922816251426433759354395.0335. A row that would take a balance beyond it is rejected. The account is left unchanged. See [Exact arithmetic](#exact-arithmetic). |
| Missing amount on a deposit or withdrawal | The transaction is rejected by the engine. |

### Transaction IDs

The specification says transaction IDs are globally unique. The engine still checks. A deposit or withdrawal that reuses the ID of an applied deposit or withdrawal is rejected. Applying a transaction twice would create or destroy money.

Only an applied transaction uses up its ID. A rejected deposit or withdrawal leaves its ID free.

This check runs before the check for a locked account. A reused ID is a fault in the data. It is reported even when a business rule would have rejected the row anyway.

### Input format

The parser is flexible about form. A row that holds the data its transaction type needs is processed.

| Case | Behavior |
|---|---|
| Header line | Optional. The first line is a header if its first field is `type`. Otherwise it is a transaction. |
| Column order | Fixed: type, client, tx, amount. Columns are read by position. |
| Whitespace around fields | Trimmed. |
| Case of the type | Ignored. `DEPOSIT` and `Deposit` are deposits. |
| Dispute, resolve and chargeback rows | May have three fields or four. |
| Fields after the fourth | Dropped. |
| Blank lines | Skipped. |
| Line endings | Unix line endings are assumed. |

### Output format

Rows are sorted by client ID. Balances are printed without trailing zeros and with at most four decimal places. The header is printed even when there are no accounts.

## Correctness

### Types

The type system carries several of the rules.

- **`Amount` can only be created by parsing.** Its field is private and there is no other constructor. An amount is therefore always greater than zero, has at most four decimal places and is within the cap. The engine cannot receive an invalid amount.
- **`Balance` is a separate type.** An amount converts into a balance. A balance never converts into an amount.
- **`ClientId` and `TxId` are distinct types.** Passing one where the other is expected does not compile.
- **`Account` derives its total.** It stores available and held funds. The total is computed from them. The three values cannot disagree.
- **`Account` guards its own rules.** Its fields are private. No caller can make held funds negative.
- **A stored deposit has its own type.** Its amount is not optional. The check for a missing amount happens once, when the deposit arrives.
- **Fallible operations return a `Result` with a named error.** Nothing returns a bare `Option` or panics.

Every account operation computes all new values first. It writes them only when every check has passed. A transaction applies in full or changes nothing.

### Exact arithmetic

Amounts are `rust_decimal` values. No float is used anywhere. Three properties of the crate needed care. Each was verified by running code against it.

1. **Deserializing a `Decimal` through the `csv` crate goes through `f64`.** That is what its default `serde` feature does. `1234567890123456.7891` becomes `1234567890123456.8`. This crate builds `rust_decimal` without any serde feature. The amount is read as text and parsed by our own code. The float path is not compiled in. It cannot come back by accident.
2. **Its standard parser rounds input that does not fit.** This crate uses `from_str_exact`. That parser returns an error instead.
3. **Its checked arithmetic can lose precision and still report success.** Near the top of its range the crate drops decimal places to make a result fit. `7922816251426433759354395.0335 + 0.0001` returns `7922816251426433759354395.034`.

The third point is the reason for the cap. `rust_decimal` has a 96 bit mantissa. Its largest mantissa is 2^96 - 1. At four decimal places that is 7922816251426433759354395.0335. Every value up to that cap is exact. `Balance` compares every result against the cap and returns an error beyond it. Tests pin the behavior on both sides of the boundary.

### Tests

| Kind | Count | Where |
|---|---|---|
| Unit tests | 105 | In each module |
| Tests that run the compiled binary | 12 | `tests/cli.rs` |

The behaviors listed under Assumptions are covered by tests with descriptive names.

The sample data is in `tests/fixtures`. Each `<name>.csv` is an input. The matching `<name>.expected.csv` is the output the binary must produce. The expected files were written by hand from the rules before the binary was run on them. They are not recorded program output.

| Fixture | Covers |
|---|---|
| `basic` | Deposits and withdrawals. One withdrawal fails. |
| `disputes` | Dispute, resolve, a second dispute, chargeback, a locked account, a negative balance |
| `ignored_rows` | Rows the specification says to ignore |
| `bad_rows` | Twelve faulty rows |
| `formatting` | Mixed case, extra whitespace, unusual number forms, quoted fields, extra fields |
| `no_header` | A file that starts with a transaction |
| `precision` | Sums that floats get wrong and a 20 digit amount |
| `empty` | An empty file |

## Safety and error handling

Nothing dangerous is done. The lint rules in `Cargo.toml` enforce that outside test code:

| Rule | Effect |
|---|---|
| `unsafe_code = "forbid"` | No unsafe code |
| `unwrap_used`, `expect_used`, `panic` | A failure must travel through a `Result` |
| `indexing_slicing` | No index that can panic |
| `arithmetic_side_effects` | No unchecked arithmetic. Every balance change goes through the checked methods. |

Errors fall into three groups.

| Group | Examples | What happens |
|---|---|---|
| Bad row | Unknown type, client that is not a number, invalid amount, too few fields | The row is skipped and logged. Processing continues. |
| Rejected transaction | See below | No balance changes. Processing continues. |
| Fatal error | The file cannot be opened or read. The output cannot be written. | The run stops with a nonzero exit code. |

A rejected transaction is logged only when it means the data is wrong: a missing amount, a reused transaction ID or a balance beyond the cap. Insufficient funds, a locked account and an ignored dispute, resolve or chargeback are ordinary outcomes. They are not logged.

One rejection cannot be caused by any input: held funds that do not cover a deposit under dispute. The account guards against it on its own. The engine never triggers that guard. It resolves or charges back only a deposit that is under dispute. Reaching the guard would mean the engine has a bug. The run then stops with an error and prints no accounts.

Rejections are sorted into three kinds: ordinary, data fault and engine fault. The function that sorts them matches on every rejection without a wildcard arm. A new kind of rejection does not compile until it is sorted.

Log lines name the record:

```
record 3: unknown transaction type "bogus"
record 7: an earlier transaction already uses this ID
```

**No partial result is ever printed.** The accounts are written only after the whole input was read and every log line was flushed. A read failure halfway through the file prints nothing. Balances from part of a file would be wrong balances.

### Dependencies

| Crate | Purpose | Notes |
|---|---|---|
| `csv` | Streaming CSV reader and writer | Suggested by the specification |
| `serde` | Derives the row struct that the reader fills | Suggested by the specification |
| `rust_decimal` | Exact decimal arithmetic | Built with only its `std` feature. The crate forbids unsafe code. |
| `thiserror` | Derives the error types | Its macro dependencies are the ones `serde` already needs. |

All dependencies were checked with `cargo deny check advisories` against the RustSec advisory database on 2026-11-14. It reported no known vulnerability and no yanked version. Every crate comes from crates.io.

`Cargo.lock` lists 57 crates. Only 18 of them take part in the build. The others are optional dependencies of `rust_decimal` features that are not enabled.

## Efficiency

### Streaming

The input is read as a stream with the `csv` crate. A row is dropped as soon as it has been applied. The file is never loaded into memory.

Measured with the release build on a laptop, single threaded:

| Input | Rows | File size | Time | Peak memory |
|---|---|---|---|---|
| Withdrawals that all fail | 10 million | 298 MB | 2.8 s | 2.5 MB |
| Deposits and withdrawals | 10 million | 287 MB | 4.5 s | 350 MB |
| Deposits only | 10 million | 277 MB | 4.7 s | 617 MB |

The first row shows the streaming. A 298 MB file needs 2.5 MB of memory when nothing has to be remembered.

### What is kept in memory

| State | Size |
|---|---|
| Accounts | At most 65536. One per client ID. |
| Applied deposits | One entry each: client, amount and dispute state |
| Applied withdrawals | The transaction ID only |

A deposit must be kept. Any deposit may be disputed later. This is the only state that grows with the input. A stored deposit takes 24 bytes with its key. The hash table adds overhead and doubles in size when it grows. The measurement above comes to about 60 bytes per deposit at the peak.

A withdrawal cannot be disputed. Only its ID is kept. The duplicate check needs it.

Transaction IDs span the `u32` range. A file with billions of deposits would not fit in memory. The store sits behind a small private type named `TransactionStore` with four methods. A store on disk could replace it without touching the rules. That was left out to keep the code simple.

### Running in a server

The specification asks what would change if the CSVs came from thousands of concurrent TCP streams.

**What already fits.** `run` takes any `Read` as input and any `Write` as output. A TCP stream can be passed where the file is passed today. The engine knows nothing about files.

**What would have to be decided.** The specification does not say whether all streams share one set of accounts.

- **Separate accounts per stream.** Each connection gets its own `Engine`. There is no shared state and nothing more to do.
- **Shared accounts.** The transactions of one client must be applied in order. The simplest correct design is one engine behind a channel. Reader tasks parse rows and send `Transaction` values. One task applies them.

**How it would scale.** Each transaction touches the account of exactly one client. The engine could be split into shards by client ID. Each shard has its own engine. A dispute goes to the shard of the client it names. That is also where the deposit is stored. One rule does not shard for free: the check for reused transaction IDs is global. It would need a shared set of IDs or a weaker rule that checks per client.

**Other changes.** The `csv` reader blocks. An async runtime would need an async CSV reader or a blocking thread per connection. Bounded channels would give backpressure. One absurd row cannot stop the service. A bad row or a rejected transaction never ends processing. The in-memory store would move to disk as described above.

## Known limits

- **Log lines name the record. They do not name the line.** The record number counts rows from 1. A header counts as a record. Blank lines do not. In a file without blank lines the record number is the line number. The `csv` crate does not report a reliable line for a row that follows blank lines.
- **Columns are read by position.** A header that lists the columns in another order is not supported.
- **A row with invalid UTF-8 is rejected whole.** This holds even when the bad byte is in a field the transaction does not need.
- **Some log text comes from dependencies.** A row with too few fields is explained in the words of the `csv` and `serde` crates.
- **All state is in memory.** See [What is kept in memory](#what-is-kept-in-memory).

## Use of AI

This solution was written with an AI coding assistant, Claude Code.

- **The decisions are mine.** The assistant listed every case the specification leaves open and offered options for each. I chose. The same holds for the design. Several of my choices went against the assistant's recommendation.
- **The assistant wrote the code and the tests.** It worked one module at a time. I reviewed each module before it was committed. Some were changed after my review.
- **Claims were verified by running code.** The three `rust_decimal` properties above were found that way. So were the limits of the line numbers in the `csv` crate.
- **Every commit carries a `Co-Authored-By` line** that names the assistant.

The full transcript of the session is provided with the submission.
