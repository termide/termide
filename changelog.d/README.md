# Changelog fragments

Each user-visible change adds its CHANGELOG entry here, in the same commit as
the change, rather than to `CHANGELOG.md`: one file per entry, so parallel
branches and sessions never edit the same lines and a merge cannot drop one.

- Put the file under the section it belongs to: `added/`, `changed/`,
  `removed/`, `fixed/`, `security/` or `performance/`.
- Name it after the entry, in kebab case: `fixed/esc-in-a-terminal-goes-to-the-shell.md`.
- Write the entry as it will read in the release notes: one bullet, a bold
  title saying the effect on the user, then the detail —
  `- **Esc in a terminal goes to the shell.** At an idle prompt …`
- A later change to the same feature before a release edits that file
  instead of adding another: the notes describe the release against the
  previous one, not the path to it.

At a release, `tools/release/changelog-fragments.sh` prints the fragments as
the sections of the new version, the release skill rewrites them into themes
and inserts them into `CHANGELOG.md`, and the fragments are removed.
