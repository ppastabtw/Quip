# Project instructions

- Treat `docs/SPEC.md` as the product source of truth and `docs/technical-plan.md` as the implementation plan.
- Target local inference and Freesolo per-user training. Privacy is a prototype direction, not a production guarantee; never use secrets or obviously sensitive personal data in project datasets.
- After changing behavior, use the relevant repository validation skill. Unit tests alone are not completion evidence. Exercise the real integration, observe it running, and report the visible result and logs. If no relevant validation skill exists, create one before claiming completion.
- Do not commit model binaries, adapters, personal data, secrets, or generated logs.
- Use the official Freesolo documentation at `https://freesolo.co/docs` for Freesolo integration decisions.
- Use `docs/freesolo-post-training-slides.md` as the primary reference for the Freesolo post-training presentation. Consult `tmp/pdfs/Freesolo_Post_Training_Slides.pdf` only when the Markdown transcription lacks needed visual or formatting context.
- On Windows, use the repository skill `$run-freesolo-flash-wsl` for Freesolo setup, training, evaluation, deployment, and export.
- Exercise Freesolo extensively, including practical limit testing. Do not conserve credits at the expense of purposeful testing, but obtain approval after dry-run and cost estimation before any paid training submission.
