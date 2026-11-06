# Working in this repository

- Use [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) to find the component that owns
  a change. Follow [CONTRIBUTING.md](CONTRIBUTING.md) for the workflow, checks,
  and commit conventions.
- Keep changes focused on the task and consistent with the surrounding code.
  Avoid unrelated renaming, restructuring, or formatting.
- Prefer readable, maintainable code. Reuse existing tools and patterns before
  adding a dependency or abstraction for a single use.
- Review the architecture for every change. Update it when ownership, flows,
  persistence, or documented paths change, and update user guides when the steps
  or behavior they describe change.
- Documentation describes the present; Git holds the history. Add investigation
  reports or temporary handoffs only when requested.
- Write project content in English, including documentation, comments, messages,
  and commits. Use uppercase Markdown filenames. In prose, use single quotation
  marks when needed and backticks for code references; avoid double or typographic
  quotation marks. Code retains the quoting required by its syntax and behavior.
- Run the relevant checks and report what passed and what you could not run.
  Automated tests use fictional credentials and local files.
- The iCloud mount is read-only. Changing that scope needs an explicit task;
  do not attempt writes against the real account on your own initiative.
- Never print or commit passwords, 2FA codes, cookies, tokens, account
  configurations, personal downloads, or the contents of `.vm/`.
- Test system changes in the dedicated VM. Do not alter other virtual machines
  or run desktop provisioning on the host.
- Create commits only when explicitly requested. Ask before publishing, deleting
  data, or rewriting shared history. Do not add automatic attribution to commits,
  comments, or documentation.
