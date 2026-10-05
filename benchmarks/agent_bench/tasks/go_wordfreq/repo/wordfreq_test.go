package wordfreq

import (
	"reflect"
	"testing"
)

func TestTokenize(t *testing.T) {
	cases := []struct {
		in   string
		want []string
	}{
		{"", []string{}},
		{"Hello, world!", []string{"hello", "world"}},
		{"don't stop", []string{"don't", "stop"}},
		{"'quoted' words", []string{"quoted", "words"}},
		{"a1 b2\tc3\n", []string{"a1", "b2", "c3"}},
		{"Ünïcödé text", []string{"ünïcödé", "text"}},
	}
	for _, c := range cases {
		t.Run(c.in, func(t *testing.T) {
			if got := Tokenize(c.in); !reflect.DeepEqual(got, c.want) {
				t.Errorf("Tokenize(%q) = %q, want %q", c.in, got, c.want)
			}
		})
	}
}

func TestCount(t *testing.T) {
	got := Count("the cat and the hat")
	want := map[string]int{"the": 2, "cat": 1, "and": 1, "hat": 1}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("Count = %v, want %v", got, want)
	}
}

func TestTopN(t *testing.T) {
	cases := []struct {
		name string
		text string
		n    int
		want []Pair
	}{
		{"empty", "", 3, []Pair{}},
		{"single", "go go go", 1, []Pair{{"go", 3}}},
		{"by count", "b a b c b a", 3, []Pair{{"b", 3}, {"a", 2}, {"c", 1}}},
		{"ties alphabetical", "pear apple fig", 3, []Pair{{"apple", 1}, {"fig", 1}, {"pear", 1}}},
		{"ties after count", "z y y x x", 3, []Pair{{"x", 2}, {"y", 2}, {"z", 1}}},
		{"truncate ties", "d c b a", 2, []Pair{{"a", 1}, {"b", 1}}},
		{"n larger than words", "one two", 5, []Pair{{"one", 1}, {"two", 1}}},
		{"case folded", "Go GO go Rust", 2, []Pair{{"go", 3}, {"rust", 1}}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := TopN(c.text, c.n); !reflect.DeepEqual(got, c.want) {
				t.Errorf("TopN(%q, %d) = %v, want %v", c.text, c.n, got, c.want)
			}
		})
	}
}
