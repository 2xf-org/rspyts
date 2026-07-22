from ..api import greet


def excited_greeting(name: str) -> str:
    """Return the generated greeting in upper case."""
    return greet(name).message.upper()
