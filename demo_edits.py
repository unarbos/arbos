def greet(name, excited=False):
    msg = "Hello, " + name
    return msg + "!" if excited else msg


def add(*nums):
    return sum(nums)


if __name__ == "__main__":
    print(greet("world", excited=True))
    print(add(2, 3, 4))
