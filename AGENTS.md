# Project instructions

- Treat `docs/SPEC.md` as the product source of truth and `docs/technical-plan.md` as the implementation plan.
- Target local inference and Freesolo per-user training. Privacy is a prototype direction, not a production guarantee; never use secrets or obviously sensitive personal data in project datasets.
- After changing behavior, use the relevant repository validation skill. Unit tests alone are not completion evidence. Exercise the real integration, observe it running, and report the visible result and logs. If no relevant validation skill exists, create one before claiming completion.
- Do not commit model binaries, adapters, personal data, secrets, or generated logs.
- Use the official Freesolo documentation at `https://freesolo.co/docs` for Freesolo integration decisions.
- Reference: `docs/freesolo-post-training-slides.md` contains the local Freesolo post-training presentation transcription for review.
- On Windows, use the repository skill `$run-freesolo-flash-wsl` for Freesolo setup, training, evaluation, deployment, and export.
