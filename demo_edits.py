def hello(name, excited=False):
    """Return a greeting for the given name, optionally excited."""
    msg = "Hello, " + name
    return msg + "!" if excited else msg


def add(*nums):
    return sum(nums)


if __name__ == "__main__":
    print(hello("world", excited=True))
    print(add(2, 3, 4))
