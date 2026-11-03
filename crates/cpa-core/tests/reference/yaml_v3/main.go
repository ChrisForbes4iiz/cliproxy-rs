// Records how gopkg.in/yaml.v3 v3.0.1 (the version CLIProxyAPI v8.0.35, ae3ee3be,
// pins in go.mod) reads a YAML edge-case corpus, for
// crates/cpa-core/tests/yaml_v3_go.rs and the parser choice in docs/YAML-PARSER.md.
//
// Each case is decoded three ways:
//   - "value":  yaml.Unmarshal into interface{} (what Go reads untyped values with),
//   - "stream": a yaml.Decoder over every document of the input,
//   - "typed":  yaml.Unmarshal into `shape`, a struct with the field kinds Go's
//     config uses (string, int, bool, float64, []string, map[string]string, struct).
//
// A value is a flat pre-order list of [type, text] tokens, so int, float, string
// and timestamp stay apart: ["null",""], ["bool","true"], ["int","123"] (decimal,
// any width), ["float","1.5"] (strconv 'g', "+Inf", "-Inf", "NaN"), ["str","x"],
// ["timestamp","<RFC 3339>"], ["seq","<n>"] and ["map","<n>"] followed by their n
// items or n key and value token runs. Go maps have no order, so map entries are
// sorted by their key tokens (see key); the Rust test sorts the same way.
//
// The cases are the corpus files (fixtures/yaml_v3/*.yaml, read as bytes), every
// "yaml" string in the other fixtures/*_go.json files of cpa-core (config.yaml-shaped
// inputs other tests already replay), and the reference's config.example.yaml. See
// README.md for the command.
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"gopkg.in/yaml.v3"
)

type nested struct {
	Name    string `yaml:"name"`
	Enabled bool   `yaml:"enabled"`
}

// shape mirrors the field kinds of Go's internal/config.Config.
type shape struct {
	Host         string            `yaml:"host"`
	Port         int               `yaml:"port"`
	Debug        bool              `yaml:"debug"`
	APIKeys      []string          `yaml:"api-keys"`
	ProxyURL     string            `yaml:"proxy-url"`
	RequestRetry int               `yaml:"request-retry"`
	Ratio        float64           `yaml:"ratio"`
	Headers      map[string]string `yaml:"headers"`
	Nested       nested            `yaml:"nested"`
}

type result struct {
	Name string `json:"name"`
	YAML string `json:"yaml"`
	// Value is the untyped decode of the first document; Error is its error text.
	Value any    `json:"value,omitempty"`
	Error string `json:"error,omitempty"`
	// Stream holds every document; StreamError is set when a document fails.
	Stream      []any  `json:"stream"`
	StreamError string `json:"stream_error,omitempty"`
	Typed       any    `json:"typed,omitempty"`
	TypedError  string `json:"typed_error,omitempty"`
}

func float(f float64) string {
	switch {
	case math.IsInf(f, 1):
		return "+Inf"
	case math.IsInf(f, -1):
		return "-Inf"
	case math.IsNaN(f):
		return "NaN"
	}
	return strconv.FormatFloat(f, 'g', -1, 64)
}

// tok is one node of a value in pre-order: its type and its text (a collection's
// text is its entry count). A flat list keeps deep documents within JSON readers'
// nesting limits.
type tok [2]string

// key orders map entries: the entry's key tokens joined with separators no text
// holds in this corpus, compared bytewise (the Rust test sorts the same way).
func key(ts []tok) string {
	var b strings.Builder
	for _, t := range ts {
		b.WriteString(t[0])
		b.WriteByte(1)
		b.WriteString(t[1])
		b.WriteByte(2)
	}
	return b.String()
}

func entries(pairs [][2]any) []tok {
	type kv struct {
		key  string
		toks []tok
	}
	list := make([]kv, 0, len(pairs))
	for _, p := range pairs {
		k := enc(p[0])
		list = append(list, kv{key(k), append(k, enc(p[1])...)})
	}
	sort.SliceStable(list, func(i, j int) bool { return list[i].key < list[j].key })
	out := []tok{{"map", strconv.Itoa(len(list))}}
	for _, e := range list {
		out = append(out, e.toks...)
	}
	return out
}

func enc(v any) []tok {
	switch x := v.(type) {
	case nil:
		return []tok{{"null", ""}}
	case bool:
		return []tok{{"bool", strconv.FormatBool(x)}}
	case int:
		return []tok{{"int", strconv.FormatInt(int64(x), 10)}}
	case int64:
		return []tok{{"int", strconv.FormatInt(x, 10)}}
	case uint64:
		return []tok{{"int", strconv.FormatUint(x, 10)}}
	case float64:
		return []tok{{"float", float(x)}}
	case string:
		return []tok{{"str", x}}
	case time.Time:
		return []tok{{"timestamp", x.Format(time.RFC3339Nano)}}
	case []any:
		out := []tok{{"seq", strconv.Itoa(len(x))}}
		for _, e := range x {
			out = append(out, enc(e)...)
		}
		return out
	case map[string]any:
		pairs := make([][2]any, 0, len(x))
		for k, e := range x {
			pairs = append(pairs, [2]any{k, e})
		}
		return entries(pairs)
	case map[any]any:
		pairs := make([][2]any, 0, len(x))
		for k, e := range x {
			pairs = append(pairs, [2]any{k, e})
		}
		return entries(pairs)
	}
	return []tok{{"unknown", fmt.Sprintf("%T %v", v, v)}}
}

func typed(s shape) any {
	keys := make([]any, 0, len(s.APIKeys))
	for _, k := range s.APIKeys {
		keys = append(keys, k)
	}
	var apiKeys any
	if s.APIKeys != nil {
		apiKeys = keys
	}
	var headers any
	if s.Headers != nil {
		h := map[string]any{}
		for k, v := range s.Headers {
			h[k] = v
		}
		headers = h
	}
	return map[string]any{
		"host":          s.Host,
		"port":          strconv.Itoa(s.Port),
		"debug":         s.Debug,
		"api-keys":      apiKeys,
		"proxy-url":     s.ProxyURL,
		"request-retry": strconv.Itoa(s.RequestRetry),
		"ratio":         float(s.Ratio),
		"headers":       headers,
		"nested":        map[string]any{"name": s.Nested.Name, "enabled": s.Nested.Enabled},
	}
}

func record(name string, text []byte) (r result) {
	r.Name, r.YAML = name, string(text)
	var v any
	if err := yaml.Unmarshal(text, &v); err != nil {
		r.Error = err.Error()
	} else {
		r.Value = enc(v)
	}
	r.Stream = []any{}
	dec := yaml.NewDecoder(bytes.NewReader(text))
	for {
		var d any
		err := dec.Decode(&d)
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			r.StreamError = err.Error()
			break
		}
		r.Stream = append(r.Stream, enc(d))
	}
	var s shape
	if err := yaml.Unmarshal(text, &s); err != nil {
		r.TypedError = err.Error()
	} else {
		r.Typed = typed(s)
	}
	return r
}

// yamlStrings collects every string stored under a "yaml" key, with a name from the
// enclosing object's "name" when it has one.
func yamlStrings(v any, prefix string, out *[][2]string, seen map[string]bool) {
	switch x := v.(type) {
	case []any:
		for i, e := range x {
			yamlStrings(e, fmt.Sprintf("%s[%d]", prefix, i), out, seen)
		}
	case map[string]any:
		if y, ok := x["yaml"].(string); ok && !seen[y] {
			seen[y] = true
			name := prefix
			if n, ok := x["name"].(string); ok {
				name = prefix + " " + n
			}
			*out = append(*out, [2]string{name, y})
		}
		keys := make([]string, 0, len(x))
		for k := range x {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			if k != "yaml" {
				yamlStrings(x[k], prefix+"."+k, out, seen)
			}
		}
	}
}

func main() {
	if len(os.Args) != 4 {
		fmt.Fprintln(os.Stderr, "usage: go run . <cpa-core tests/fixtures dir> <config.example.yaml> <out.json>")
		os.Exit(2)
	}
	fixtures, example, outPath := os.Args[1], os.Args[2], os.Args[3]
	var results []result

	corpus, err := filepath.Glob(filepath.Join(fixtures, "yaml_v3", "*.yaml"))
	if err != nil {
		panic(err)
	}
	sort.Strings(corpus)
	for _, f := range corpus {
		text, err := os.ReadFile(f)
		if err != nil {
			panic(err)
		}
		results = append(results, record("corpus/"+strings.TrimSuffix(filepath.Base(f), ".yaml"), text))
	}

	others, err := filepath.Glob(filepath.Join(fixtures, "*_go.json"))
	if err != nil {
		panic(err)
	}
	sort.Strings(others)
	seen := map[string]bool{}
	for _, f := range others {
		base := filepath.Base(f)
		if base == "yaml_v3_go.json" {
			continue
		}
		raw, err := os.ReadFile(f)
		if err != nil {
			panic(err)
		}
		var doc any
		if err := json.Unmarshal(raw, &doc); err != nil {
			panic(fmt.Sprintf("%s: %v", f, err))
		}
		var found [][2]string
		yamlStrings(doc, "fixture/"+strings.TrimSuffix(base, "_go.json"), &found, seen)
		for _, c := range found {
			results = append(results, record(c[0], []byte(c[1])))
		}
	}

	text, err := os.ReadFile(example)
	if err != nil {
		panic(err)
	}
	results = append(results, record("reference/config.example.yaml", text))

	// One case per line keeps the file small and its diffs readable.
	var buf bytes.Buffer
	buf.WriteString("[\n")
	for i, r := range results {
		var line bytes.Buffer
		enc := json.NewEncoder(&line)
		enc.SetEscapeHTML(false)
		if err := enc.Encode(r); err != nil {
			panic(err)
		}
		buf.Write(bytes.TrimSuffix(line.Bytes(), []byte("\n")))
		if i+1 < len(results) {
			buf.WriteString(",")
		}
		buf.WriteString("\n")
	}
	buf.WriteString("]\n")
	if err := os.WriteFile(outPath, buf.Bytes(), 0o644); err != nil {
		panic(err)
	}
	fmt.Fprintf(os.Stderr, "%d cases\n", len(results))
}
