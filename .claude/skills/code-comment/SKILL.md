---
name: code-comment
description: Code comment policy. Consult when writing/modifying code or asked to clean up comments. Eliminate unnecessary comments; record "why not".
---

# Comment Policy

Single test: **would you write this comment if creating the file from scratch?** If not, don't write it when modifying either.

**Never write:** restating the code (`// open the file`), change descriptions (`// added X` — that's git's job), reviewer justifications, section dividers, empty TODOs.

**Write only** what the code can't express — especially **why not**: why the obvious alternative was rejected.

- `// loop copy instead of memcpy: src/dst may overlap`
- `// re-check looks redundant but required: Y mutates this asynchronously`

If a future "simplification" would break the code, write a why-not there. Straightforward code needs no comment.

## Cleanup mode

For the target files (default: current diff): delete violating comments; rewrite what-comments that hide a reason into why-not form; add why-nots where implicit constraints are dangerous. When in doubt, delete — git can restore.
