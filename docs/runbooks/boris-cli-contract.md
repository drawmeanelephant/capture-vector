# Run the Boris CLI contract probe

Use this probe when you change a Boris argv shape in `crates/cv-engine` or
`crates/cv-publish`. The contract is `fixtures/boris-cli-contract.json`. It is
generated evidence from the real pinned Boris binary, not a hand-written Boris
fixture.

## Understand the two gates

- Static gate: `cargo test --workspace --locked` compares the argv shapes that
  the product constructs with the checked-in contract. It does not spawn Boris.
- Black-box gate: an opt-in test runs every contract shape against the real
  pinned Boris binary in a fresh copy of `fixtures/corpus-src/happy`.

Run ids and output paths in the contract are canonical representatives. The
flags, their order, the exit code, and the first error line are the contract.

## Run the black-box probe

1. Build or locate Boris 0.9.0 at vcs `66efcfb`.
2. Run the probe:

   ```sh
   CV_BORIS_CONTRACT=1 \
   CV_BORIS_BIN="$HOME/.local/src/boris-0.9.0/zig-out/bin/boris" \
   cargo test -p cv-engine --test boris_cli_contract \
     black_box_boris_cli_contract --locked -- --exact --ignored --nocapture
   ```

The probe prints one line per shape. `exit=2` is Boris's usage-error class.
Read `error_line` before interpreting it: `unexpected argument: publish` is a
shape refusal, while `profile declares no nostr section` means the parser
accepted the Nostr command family and rejected the selected profile.

The publication-family rows never publish. Nostr rows use missing plan or
bundle artifacts and stop at the artifact boundary. Standard.site rows use the
happy fixture profile, which is invalid for that family. No row contacts a
relay or PDS.

## Regenerate the contract

Regenerate the contract after an intentional argv change or a Boris pin bump:

```sh
CV_BORIS_CONTRACT=1 \
CV_BORIS_CONTRACT_WRITE=1 \
CV_BORIS_BIN="$HOME/.local/src/boris-0.9.0/zig-out/bin/boris" \
cargo test -p cv-engine --test boris_cli_contract \
  black_box_boris_cli_contract --locked -- --exact --ignored --nocapture
```

Then run the ordinary gates:

```sh
cargo test --workspace --locked
./scripts/lint.sh
```

Never substitute a fake Boris for `CV_BORIS_BIN`. If the pinned binary is
absent, build it as `AGENTS.md` describes or stop.
