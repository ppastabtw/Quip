# Dataset attribution

The compiled Quip Flash corpus is marked `research_only`.

- UCI SMS Spam Collection, Almeida and Hidalgo, is licensed under CC BY 4.0. Only messages labeled ham are used, and only in training.
- MultiLexNorm, van der Goot et al., is licensed under CC BY 4.0. The English train partition is used for training and the English test partition is used for held-out evaluation.
- JFLEG, Napoles et al., is licensed under CC BY-NC-SA 4.0. Only the human-corrected test data is used, and only in evaluation.

The combined corpus must be used only for noncommercial research, with attribution and share-alike obligations preserved. Raw downloads are not redistributed. Exact source revisions, checksums, and permitted uses are recorded in `source_manifest.json`.

The original GitHub Typo Corpus v1.0.0 source was not used because its published download bucket is no longer available. MultiLexNorm replaces it with an accessible, versioned, human-annotated source whose official train and test partitions support leakage-safe compilation.
