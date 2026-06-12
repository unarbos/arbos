def bubble_sort(arr):
    a = list(arr)
    n = len(a)
    for i in range(n - 1):
        swapped = False
        for j in range(n - 1 - i):
            if a[j] > a[j + 1]:
                a[j], a[j + 1] = a[j + 1], a[j]
                swapped = True
        if not swapped:
            break
    return a


if __name__ == "__main__":
    sample = [5, 2, 9, 1, 5, 6, 0, -3]
    print("input: ", sample)
    print("sorted:", bubble_sort(sample))
