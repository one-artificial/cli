---
name: no-sed-for-refactoring
description: Never use sed for code modifications — it destroys files. Use Edit tool with targeted replacements.
type: feedback
---

Never use sed, awk, or batch find-replace commands to modify source code. The Edit tool exists for a reason — it validates the old_string exists before replacing. sed is destructive and has destroyed entire files in this project.

**Why:** A batch sed wiped crates/one-tui/src/commands.rs to 2 lines by matching patterns inside code, not just comments. Required git checkout to restore.

**How to apply:** For comment cleanup or renaming across files, read each file first, identify the exact lines, use Edit with precise old_string/new_string pairs, and verify compilation after each batch of 3-5 edits.
