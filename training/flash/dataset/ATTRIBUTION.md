# Dataset attribution

Quip V0 uses the English portion of MASSIVE 1.1 as its sole seed dataset.
MASSIVE is provided by Amazon under the Creative Commons Attribution 4.0
International license.

The compiler consumes full English utterances, uses the official train
partition for Quip training, and uses the official dev and test partitions for
Quip evaluation. It derives bounded correction windows locally and does not
redistribute the raw archive.

The exact archive URL, revision, archive member, license, and hashes live only
in `source_manifest.json`. Dataset policy lives only in
`docs/training-data-contract.md`.

Please cite the MASSIVE and SLURP papers requested by the dataset authors when
publishing results based on this data.
