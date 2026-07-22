from typing import assert_type

from example import Greeting, excited_greeting, greet

assert_type(greet("Ada"), Greeting)
assert_type(excited_greeting("Ada"), str)
