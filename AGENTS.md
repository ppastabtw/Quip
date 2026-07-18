# Project instructions

- Treat `docs/SPEC.md` as the product source of truth and `docs/technical-plan.md` as the implementation plan.
- Keep inference, raw drafts, ambient context, and the primary personal record store local to the Mac. Per-user adapter training uses Freesolo with compact confirmed examples only.
- After changing behavior, use the relevant repository validation skill. Unit tests alone are not completion evidence. Exercise the real integration, observe it running, and report the visible result and logs. If no relevant validation skill exists, create one before claiming completion.
- Do not commit model binaries, adapters, personal data, secrets, or generated logs.
- Use the official Freesolo documentation at `https://freesolo.co/docs` for Freesolo integration decisions.
