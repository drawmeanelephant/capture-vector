# Verify native windows and IME on macOS

## Run the native probe

Use a real macOS desktop with the US keyboard input source enabled.
The probe requires Python 3, Swift, and the repository's Rust toolchain.

The probe brings its windows to the foreground and sends native keys.
Leave the keyboard and mouse alone until the probe finishes.
If the test app loses focus, the probe stops without sending the next key.

1. Grant your terminal Accessibility, Automation for System Events, and
   Screen Recording access in **System Settings** if those permissions are missing.
2. Run this command from the repository root:

   ```sh
   python3 spike/macos-probe/run.py
   ```

3. Confirm that the output contains nine `PASS` lines and exits successfully.
4. Open the artifact directory printed at the end.
   Inspect `us-preedit.png`, `us-committed.png`, `dirty-close.png`, and
   `pinned-companion.png`.

The probe builds and launches the real `cv-app` against a copy of
`fixtures/corpus-src/happy`. Its config, Rune files, screenshots, and
accessibility trees stay in a new `target/macos-probe/run-*` directory.
It restores the previous input source and requests native Quit
so the app stops any Boris watch session. Forced termination is a fallback.
It does not execute or change `ime_probe_scratch.rs`.

`native.swift` reads the app's accessibility tree and sends native input.
`reference.swift` supplies an independent AppKit `NSTextView` control.
The control confirms that Escape commits the literal accent for US dead keys.
That behavior is not Japanese composition cancellation.

These PNGs are evidence from the local desktop, not canonical goldens.
Lavapipe remains the canonical PNG gate.
The recorded results and limits are in [the macOS findings](../FINDINGS.md#2026-11-23--issue-26-macos-native-probes).

## Check the remaining IME cases manually

Use an enabled Japanese input source for this check.
Automatic input-source enablement did not give a reliable replay in this environment.
Repeat the steps in a standard native text editor if an outcome is unclear.

1. Make a writable copy of `fixtures/corpus-src/happy`.
   Launch the editor against that copy:

   ```sh
   CV_EDIT=content/index.md cargo run -p cv-app -- --project <corpus-copy>
   ```

2. Enter a Latin prefix, such as `café`, in the body.
3. Switch to Japanese Hiragana input and type `kyou`.
   Check that marked text appears at the caret.
   The draft sketch must not include uncommitted marked text.
4. Open the candidate list with Space.
   Check that the list appears beside the marked text, not at another window.
   Select a candidate and commit it.
   The editor and the draft sketch must show the selected text exactly once.
5. Start another composition and cancel it with Escape.
   Check that surrounding committed text survives.
6. Repeat at a caret after multibyte text, with a selected word, and after
   switching between the primary window and the editor.
7. Save the page and check the file's UTF-8 text.
8. Record the OS version, input source, steps, selected text, and screenshots.
   Leave untested cases marked unproven.

## Check native windows in the Windows VM

Use a writable copy of the happy corpus.
Enable a Japanese IME in Windows before the IME steps.
VM keyboard capture must send the keys to the guest.

1. Build with `cargo build -p cv-app --locked`.
2. Set `$env:CV_EDIT = 'content/index.md'` in PowerShell.
3. Start `.\target\debug\cv-app.exe --project <corpus-copy>`.
   Confirm that the primary window and the editor open.
4. Change the body, then close the editor with the native close button.
   Confirm that it reopens with the edits and a save, discard, or keep-editing prompt.
5. Choose **Keep editing**.
   Confirm that the edits remain.
6. Close again and choose **Save and close**.
   Confirm that the file contains the edits and the editor closes.
7. Select a page in the primary window and press Enter.
   Change the primary selection and confirm that the companion stays pinned.
8. Open the same page again.
   Confirm that no duplicate companion appears.
9. Close the companion, then reopen the same page.
   Confirm that a new native window opens.
10. Repeat the remaining IME cases above using Windows keyboard capture.
11. Run `cargo test --workspace --locked` and attach the native-test evidence.

The macOS Swift probe does not run on Windows.
Passing headless tests does not prove the guest's native windows or IME.
