import random
import tempfile
import zipfile
from pathlib import Path

from dataset_compiler import sources
from dataset_compiler.contract import word_count
from tests.dataset_compiler.helpers import candidate


def test_uci_parser_keeps_ham_only():
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "sms.zip"
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr(
                "SMSSpamCollection",
                "ham\tcall me\nspam\tbuy now\n"
                + "\n".join(f"ham\tfixture message {index}" for index in range(4000)),
            )
        rows = sources.parse_uci_ham(path)
    assert rows[0][1] == "call me"
    assert "buy now" not in {text for _, text in rows}


def test_multilexnorm_parser_preserves_sentence_boundaries():
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "data.norm"
        path.write_text("u\tyou\nthere\tthere\n\n" * 1000, encoding="utf-8")
        sentences = sources.parse_multilexnorm(path)
    assert len(sentences) == 1000
    assert sentences[0] == [("u", "you"), ("there", "there")]


def test_quota_selection_is_seeded_and_enforces_singles():
    candidates = [candidate(index, f"word{index}") for index in range(20)]
    candidates += [candidate(100 + index, f"call me {index}") for index in range(20)]
    first = sources.choose_candidates(
        candidates,
        count=10,
        single_count=3,
        rng=random.Random(42),
    )
    second = sources.choose_candidates(
        candidates,
        count=10,
        single_count=3,
        rng=random.Random(42),
    )
    assert first == second
    assert sum(word_count(item.text) == 1 for item in first) == 3


def test_split_family_names_are_disjoint():
    train = sources.multilexnorm_candidates(
        [[("u", "you"), ("there", "there")]], split="train", action="replace"
    )
    test = sources.multilexnorm_candidates(
        [[("u", "you"), ("there", "there")]], split="test", action="replace"
    )
    assert {item.family_id for item in train}.isdisjoint(item.family_id for item in test)
