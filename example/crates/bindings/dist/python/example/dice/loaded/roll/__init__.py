"""Generated from the Rust application API."""

__all__ = [
    "RollResult",
    "DEFAULT_FAVORED_VALUE",
    "DiceCup",
    "roll_dice",
]

def __getattr__(
    name: str,
    _exports: dict[str, tuple[str, str]] = {
        "RollResult": (".models", "RollResult"),
        "DEFAULT_FAVORED_VALUE": (".api", "DEFAULT_FAVORED_VALUE"),
        "DiceCup": (".api", "DiceCup"),
        "roll_dice": (".api", "roll_dice"),
    },
    _package: str = __name__,
) -> object:
    from builtins import AttributeError as builtin_attribute_error
    from builtins import KeyError as builtin_key_error
    from builtins import getattr as builtin_getattr
    from importlib import import_module
    from sys import modules

    try:
        module_name, member_name = _exports[name]
    except builtin_key_error:
        raise builtin_attribute_error(name) from None
    value = builtin_getattr(import_module(module_name, _package), member_name)
    modules[_package].__dict__[name] = value
    return value


def __dir__(
    _exports: tuple[str, ...] = tuple(__all__),
    _package: str = __name__,
) -> list[str]:
    from builtins import set as builtin_set
    from builtins import sorted as builtin_sorted
    from sys import modules

    return builtin_sorted(builtin_set(modules[_package].__dict__) | builtin_set(_exports))
