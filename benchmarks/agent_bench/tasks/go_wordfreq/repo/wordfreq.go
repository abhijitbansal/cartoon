// Package wordfreq counts words and reports the most frequent ones.
package wordfreq

import (
	"sort"
	"strings"
	"unicode"
)

// Pair is a word and how often it occurred.
type Pair struct {
	Word  string
	Count int
}

// Tokenize splits text into lowercase words. Letters, digits and inner
// apostrophes ("don't") belong to a word; everything else separates words.
func Tokenize(text string) []string {
	fields := strings.FieldsFunc(text, func(r rune) bool {
		return !unicode.IsLetter(r) && !unicode.IsDigit(r) && r != '\''
	})
	words := make([]string, 0, len(fields))
	for _, f := range fields {
		f = strings.Trim(f, "'")
		if f != "" {
			words = append(words, strings.ToLower(f))
		}
	}
	return words
}

// Count returns how often each word occurs.
func Count(text string) map[string]int {
	counts := make(map[string]int)
	for _, w := range Tokenize(text) {
		counts[w]++
	}
	return counts
}

// TopN returns the n most frequent words, most frequent first; ties are
// broken alphabetically so the result is deterministic.
func TopN(text string, n int) []Pair {
	counts := Count(text)
	pairs := make([]Pair, 0, len(counts))
	for w, c := range counts {
		pairs = append(pairs, Pair{w, c})
	}
	sort.Slice(pairs, func(i, j int) bool {
		if pairs[i].Count != pairs[j].Count {
			return pairs[i].Count > pairs[j].Count
		}
		return pairs[i].Word > pairs[j].Word
	})
	if n < len(pairs) {
		pairs = pairs[:n]
	}
	return pairs
}
