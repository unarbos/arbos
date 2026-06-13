package main

import "fmt"

// BubbleSort sorts s in ascending order in place using the bubble sort
// algorithm. After each pass the largest unsorted element bubbles to its
// final position, so the unsorted prefix shrinks by one each pass. The
// swapped flag lets it exit early once the slice is already ordered.
func BubbleSort(s []int) {
	for end := len(s) - 1; end > 0; end-- {
		swapped := false
		for i := 0; i < end; i++ {
			if s[i] > s[i+1] {
				s[i], s[i+1] = s[i+1], s[i]
				swapped = true
			}
		}
		if !swapped {
			return
		}
	}
}

func main() {
	data := []int{5, 2, 9, 1, 5, 6, 0, -3, 8, 4}
	fmt.Println("before:", data)
	BubbleSort(data)
	fmt.Println("after: ", data)
}
