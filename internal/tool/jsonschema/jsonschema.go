// Package jsonschema derives a JSON Schema from a Go argument struct. It is the
// engine behind tool-schema codegen (ADR-0004): a generator reflects each
// tool's arg type at build time and writes the resulting schema into committed
// source, so the schema can never drift from the handler's actual arguments and
// CI fails if someone edits the struct without regenerating. Nothing reflects
// at runtime — the kernel ships the generated literals.
//
// Output is deterministic: encoding/json sorts object keys, and the only
// order-bearing field (required) follows struct field declaration order, so
// regenerating an unchanged type produces byte-identical bytes.
package jsonschema

import (
	"encoding/json"
	"fmt"
	"reflect"
	"strings"
)

// Reflect returns the JSON Schema for a struct type. The top-level type must be
// a struct; field types may be string, bool, integer, number, or []string —
// the shapes the kernel's built-in tools use. A field is required unless its
// json tag carries ",omitempty". Descriptions come from a `desc` struct tag.
func Reflect(t reflect.Type) (json.RawMessage, error) {
	for t.Kind() == reflect.Pointer {
		t = t.Elem()
	}
	if t.Kind() != reflect.Struct {
		return nil, fmt.Errorf("jsonschema: top-level type must be a struct, got %s", t.Kind())
	}
	schema, err := objectSchema(t)
	if err != nil {
		return nil, err
	}
	return json.Marshal(schema)
}

// objectSchema builds the JSON Schema object for a struct: its exported fields
// become properties (keyed by json name), a non-omitempty field is required, and
// a `desc` tag becomes the property description. It recurses through fieldSchema
// so nested structs and struct slices (e.g. edit's edits[]) are supported.
func objectSchema(t reflect.Type) (map[string]any, error) {
	props := map[string]any{}
	var required []string
	for i := 0; i < t.NumField(); i++ {
		f := t.Field(i)
		if !f.IsExported() {
			continue
		}
		name, omit := jsonName(f)
		if name == "-" {
			continue
		}
		ps, err := fieldSchema(f.Type)
		if err != nil {
			return nil, fmt.Errorf("jsonschema: field %s: %w", f.Name, err)
		}
		if d := f.Tag.Get("desc"); d != "" {
			ps["description"] = d
		}
		props[name] = ps
		if !omit {
			required = append(required, name)
		}
	}
	schema := map[string]any{
		"type":                 "object",
		"properties":           props,
		"additionalProperties": false,
	}
	if len(required) > 0 {
		schema["required"] = required
	}
	return schema, nil
}

func fieldSchema(t reflect.Type) (map[string]any, error) {
	for t.Kind() == reflect.Pointer {
		t = t.Elem()
	}
	switch t.Kind() {
	case reflect.String:
		return map[string]any{"type": "string"}, nil
	case reflect.Bool:
		return map[string]any{"type": "boolean"}, nil
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return map[string]any{"type": "integer"}, nil
	case reflect.Float32, reflect.Float64:
		return map[string]any{"type": "number"}, nil
	case reflect.Slice:
		items, err := fieldSchema(t.Elem())
		if err != nil {
			return nil, fmt.Errorf("slice element: %w", err)
		}
		return map[string]any{"type": "array", "items": items}, nil
	case reflect.Struct:
		return objectSchema(t)
	default:
		return nil, fmt.Errorf("unsupported field kind %s", t.Kind())
	}
}

func jsonName(f reflect.StructField) (name string, omitempty bool) {
	tag := f.Tag.Get("json")
	if tag == "" {
		return f.Name, false
	}
	parts := strings.Split(tag, ",")
	name = parts[0]
	if name == "" {
		name = f.Name
	}
	for _, p := range parts[1:] {
		if p == "omitempty" {
			omitempty = true
		}
	}
	return name, omitempty
}
