### Changed

- **Pull requests run a quick CI profile and the merge queue runs the full
  pipeline.** A pull request runs unit tests on one runner, clippy, fmt,
  docs, the script guards and semver; the merge queue, pushes to main,
  manual dispatches and any other event run every leg as before, and ci-ok
  refuses a missing plan, a missing job and any unexpected skip (#155).
