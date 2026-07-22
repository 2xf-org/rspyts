from example import Greeting, excited_greeting, greet


def test_generated_and_authored_python() -> None:
    assert greet("Ada") == Greeting(message="Hello, Ada!")
    assert excited_greeting("Ada") == "HELLO, ADA!"
