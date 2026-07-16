"""English/French selection for Chatterbox multilingual synthesis."""

from yapper_tts.language import effective_cfg_weight, resolve_language, retry_cfg_weight


def test_explicit_language_is_unchanged() -> None:
    assert resolve_language("fr", "This is English") == "fr"
    assert resolve_language("en", "Ceci est français") == "en"


def test_auto_detects_french_words_and_accents() -> None:
    assert resolve_language("auto", "Bonjour, ceci est une phrase en français.") == "fr"
    assert resolve_language("auto", "L'élève écoute une très belle chanson.") == "fr"


def test_auto_detects_english_and_defaults_ambiguous_text_to_english() -> None:
    assert resolve_language("auto", "This is a sentence with a technical API.") == "en"
    assert resolve_language("auto", "Yapper 4070") == "en"


def test_cross_language_cfg_is_capped_and_retry_does_not_raise_it() -> None:
    assert effective_cfg_weight("fr", None, 0.5) == 0.2
    assert retry_cfg_weight("fr", None, 0.2) == 0.2
    assert effective_cfg_weight("fr", "fr", 0.5) == 0.5
    assert effective_cfg_weight("en", None, 0.5) == 0.5
