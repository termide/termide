//! Generate Mermaid class diagrams from source code.
//!
//! Rust, Python, TypeScript/TSX/JSX, Go, Java, C, C++, Ruby, and PHP go through
//! dedicated rich tree-sitter extractors that recover visibility, field/
//! attribute types, method signatures, enum variants, module-level free items,
//! and inheritance/realization/composition relationships. Remaining tree-sitter
//! languages (bash, Haskell, Nix, …) fall back to a name-only extractor
//! ([`generic`]) that still yields type boxes with methods plus a `<<module>>`
//! box of top-level functions.

mod c;
mod cpp;
mod generic;
mod go;
mod java;
mod model;
mod php;
mod python;
mod ruby;
mod rust;
mod typescript;

use std::path::Path;

/// Generate a Mermaid `classDiagram` from source code.
///
/// Dispatches to a language-specific rich extractor (see the module docs) or
/// the name-only [`generic`] fallback. Returns `None` when the language is
/// unsupported or the source has nothing to diagram.
pub fn generate_class_diagram(
    source: &str,
    language: Option<&str>,
    file_path: Option<&Path>,
) -> Option<String> {
    match resolve_language(language, file_path).as_deref() {
        Some("rust") => rust::generate(source, file_path),
        Some("python") => python::generate(source, file_path),
        Some("typescript") | Some("javascript") => typescript::generate(source, file_path, false),
        // TSX/JSX need the JSX-aware grammar.
        Some("tsx") | Some("jsx") => typescript::generate(source, file_path, true),
        Some("go") => go::generate(source, file_path),
        Some("java") => java::generate(source, file_path),
        Some("c") => c::generate(source, file_path),
        Some("cpp") => cpp::generate(source, file_path),
        Some("ruby") => ruby::generate(source, file_path),
        Some("php") => php::generate(source, file_path),
        _ => generic::generate(source, language, file_path),
    }
}

/// Resolve the language: prefer the caller-provided name, else map the file
/// extension for the rich-extractor languages.
fn resolve_language(language: Option<&str>, file_path: Option<&Path>) -> Option<String> {
    if let Some(l) = language {
        return Some(l.to_string());
    }
    let ext = file_path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())?;
    match ext {
        "rs" => Some("rust".to_string()),
        "py" => Some("python".to_string()),
        "ts" | "mts" | "cts" => Some("typescript".to_string()),
        "tsx" => Some("tsx".to_string()),
        "js" | "mjs" | "cjs" => Some("javascript".to_string()),
        "jsx" => Some("jsx".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust(src: &str) -> String {
        generate_class_diagram(src, Some("rust"), None).expect("expected a diagram")
    }

    #[test]
    fn no_diagram_for_empty_source() {
        assert!(generate_class_diagram("", Some("rust"), None).is_none());
    }

    #[test]
    fn free_functions_appear_in_module_box() {
        // Regression: previously a file of only free functions produced no
        // diagram at all, so public functions were invisible.
        let out = rust("pub fn parse(s: &str) -> usize { 0 }\nfn helper() {}\n");
        assert!(out.contains("<<module>>"), "got:\n{out}");
        assert!(out.contains("+parse(&str) usize"), "got:\n{out}");
        assert!(out.contains("-helper()"), "got:\n{out}");
    }

    #[test]
    fn struct_fields_carry_types_and_visibility() {
        let out = rust("pub struct Circle { pub radius: f64, center: Point }");
        assert!(out.contains("class Circle {"), "got:\n{out}");
        assert!(out.contains("+radius: f64"), "got:\n{out}");
        assert!(out.contains("-center: Point"), "got:\n{out}");
    }

    #[test]
    fn impl_methods_have_signatures() {
        let out = rust("struct Circle;\nimpl Circle {\n    pub fn scale(&self, k: f64) -> Circle { *self }\n    fn hidden(&self) {}\n}\n");
        assert!(out.contains("+scale(f64) Circle"), "got:\n{out}");
        assert!(out.contains("-hidden()"), "got:\n{out}");
    }

    #[test]
    fn trait_is_stereotyped_with_methods() {
        let out =
            rust("pub trait Draw {\n    fn draw(&self);\n    fn area(&self) -> f64 { 0.0 }\n}\n");
        assert!(out.contains("class Draw {"), "got:\n{out}");
        assert!(out.contains("<<trait>>"), "got:\n{out}");
        assert!(out.contains("+draw()"), "got:\n{out}");
        assert!(out.contains("+area() f64"), "got:\n{out}");
    }

    #[test]
    fn enum_variants_and_stereotype() {
        let out =
            rust("pub enum Shape {\n    Unit,\n    Pair(f64, f64),\n    Named { side: f64 },\n}\n");
        assert!(out.contains("<<enum>>"), "got:\n{out}");
        assert!(out.contains("Unit"), "got:\n{out}");
        assert!(out.contains("Pair(f64, f64)"), "got:\n{out}");
        assert!(out.contains("Named(side: f64)"), "got:\n{out}");
        // Braces must never leak into member lines (they close the class block).
        assert!(!out.contains("Named {"), "braces leaked:\n{out}");
    }

    #[test]
    fn trait_impl_creates_realization_edge() {
        let out = rust("struct Circle;\ntrait Draw { fn draw(&self); }\nimpl Draw for Circle { fn draw(&self) {} }\n");
        assert!(out.contains("Draw <|.. Circle"), "got:\n{out}");
    }

    #[test]
    fn field_composition_edge_only_for_local_types() {
        // `center: Point` where Point is declared locally -> composition edge;
        // `radius: f64` (primitive) and external types produce no edge.
        let out = rust("struct Point;\nstruct Circle { center: Point, radius: f64 }\n");
        assert!(out.contains("Circle *-- Point"), "got:\n{out}");
        assert!(
            !out.contains("*-- f64"),
            "primitive should not compose:\n{out}"
        );
    }

    #[test]
    fn module_box_lists_const_and_type_alias() {
        let out = rust("pub(crate) const PI: f64 = 3.14;\npub type Meters = f64;\n");
        assert!(out.contains("<<module>>"), "got:\n{out}");
        assert!(out.contains("~const PI: f64"), "got:\n{out}");
        assert!(out.contains("+type Meters"), "got:\n{out}");
    }

    #[test]
    fn module_box_named_after_file() {
        let out = generate_class_diagram(
            "pub fn go() {}",
            Some("rust"),
            Some(Path::new("/src/shape.rs")),
        )
        .unwrap();
        assert!(out.contains("class shape {"), "got:\n{out}");
    }

    #[test]
    fn unsupported_language_uses_generic_fallback() {
        // Go has no rich extractor -> name-only boxes via the outline.
        let out = generate_class_diagram(
            "package main\ntype Animal struct {}\nfunc (a Animal) Speak() {}\n",
            Some("go"),
            None,
        );
        assert!(
            out.is_some(),
            "generic fallback should still produce a diagram"
        );
    }

    #[test]
    fn generic_fallback_lists_top_level_functions() {
        // A function-only file (no types) must still produce a module box,
        // instead of an empty "no symbols" result.
        let out = generate_class_diagram(
            "package main\nfunc Hello() {}\nfunc World() {}\n",
            Some("go"),
            None,
        )
        .expect("function-only file should still diagram");
        assert!(out.contains("<<module>>"), "got:\n{out}");
        assert!(out.contains("+Hello()"), "got:\n{out}");
        assert!(out.contains("+World()"), "got:\n{out}");
    }

    #[test]
    fn tsx_with_jsx_diagrams_via_tsx_grammar() {
        // Regression: `.tsx` used to fall through to the generic path with the
        // plain-TS grammar and failed on JSX, so no diagram opened.
        let src = "export interface Props { title: string; }\n\
                   export function App(props: Props) { return <div>{props.title}</div>; }\n\
                   export const VERSION: string = \"1\";\n";
        let out = generate_class_diagram(src, Some("tsx"), Some(Path::new("App.tsx")))
            .expect("tsx should diagram");
        assert!(out.contains("class Props {"), "got:\n{out}");
        assert!(out.contains("<<interface>>"), "got:\n{out}");
        assert!(out.contains("<<module>>"), "got:\n{out}");
        assert!(out.contains("+App(Props)"), "got:\n{out}");
        assert!(out.contains("+VERSION: string"), "got:\n{out}");
    }

    // --- Python ---

    fn python(src: &str) -> String {
        generate_class_diagram(src, Some("python"), None).expect("expected a diagram")
    }

    #[test]
    fn python_free_functions_and_consts_in_module_box() {
        let out = python("PI: float = 3.14\ndef area(r: float) -> float:\n    return r\n");
        assert!(out.contains("<<module>>"), "got:\n{out}");
        assert!(out.contains("+PI: float"), "got:\n{out}");
        assert!(out.contains("+area(float) float"), "got:\n{out}");
    }

    #[test]
    fn python_class_methods_attributes_and_inheritance() {
        let out = python(
            "class Animal(Base):\n    kind: str = \"?\"\n    def speak(self, loud: bool) -> None:\n        pass\n    def _hidden(self):\n        pass\n",
        );
        assert!(out.contains("class Animal {"), "got:\n{out}");
        assert!(out.contains("+kind: str"), "got:\n{out}");
        assert!(out.contains("+speak(bool) None"), "got:\n{out}");
        assert!(out.contains("-_hidden()"), "got:\n{out}");
        assert!(out.contains("Base <|-- Animal"), "got:\n{out}");
    }

    // --- TypeScript ---

    fn ts(src: &str) -> String {
        generate_class_diagram(src, Some("typescript"), None).expect("expected a diagram")
    }

    #[test]
    fn ts_module_items_and_type_alias() {
        let out = ts(
            "export const TAU: number = 6.28;\nexport function area(r: number): number { return r; }\nexport type Meters = number;\n",
        );
        assert!(out.contains("<<module>>"), "got:\n{out}");
        assert!(out.contains("+TAU: number"), "got:\n{out}");
        assert!(out.contains("+area(number) number"), "got:\n{out}");
        assert!(out.contains("+type Meters"), "got:\n{out}");
    }

    #[test]
    fn ts_class_fields_visibility_and_heritage() {
        let out = ts(
            "class Circle extends Shape implements Draw {\n    public radius: number = 0;\n    private center: Point;\n    draw(): void {}\n}\n",
        );
        assert!(out.contains("class Circle {"), "got:\n{out}");
        assert!(out.contains("+radius: number"), "got:\n{out}");
        assert!(out.contains("-center: Point"), "got:\n{out}");
        assert!(out.contains("+draw() void"), "got:\n{out}");
        assert!(out.contains("Shape <|-- Circle"), "got:\n{out}");
        assert!(out.contains("Draw <|.. Circle"), "got:\n{out}");
    }

    #[test]
    fn ts_interface_and_enum_stereotypes() {
        let out = ts(
            "export interface Draw { draw(): void; area(): number; }\nexport enum Color { Red, Green }\n",
        );
        assert!(out.contains("class Draw {"), "got:\n{out}");
        assert!(out.contains("<<interface>>"), "got:\n{out}");
        assert!(out.contains("+draw() void"), "got:\n{out}");
        assert!(out.contains("<<enum>>"), "got:\n{out}");
        assert!(out.contains("Red"), "got:\n{out}");
    }

    // --- Go ---

    #[test]
    fn go_structs_interfaces_methods_and_module() {
        let src = "package main\n\
                   type Shape interface { Area() float64 }\n\
                   type Point struct { X float64 }\n\
                   type Circle struct { Radius float64; center Point }\n\
                   func (c Circle) Area() float64 { return 0 }\n\
                   func Bounding(s Shape) Point { return Point{} }\n\
                   const Pi = 3.14\n";
        let out = generate_class_diagram(src, Some("go"), None).expect("go diagram");
        assert!(
            out.contains("class Shape {") && out.contains("<<interface>>"),
            "got:\n{out}"
        );
        assert!(out.contains("+Area() float64"), "got:\n{out}");
        assert!(out.contains("+Radius: float64"), "got:\n{out}");
        assert!(out.contains("-center: Point"), "got:\n{out}");
        assert!(
            out.contains("<<module>>") && out.contains("+Bounding(Shape) Point"),
            "got:\n{out}"
        );
        assert!(out.contains("Circle *-- Point"), "got:\n{out}");
    }

    // --- Java ---

    #[test]
    fn java_class_fields_methods_and_heritage() {
        let src = "public interface Draw { void draw(); }\n\
                   public class Circle extends Shape implements Draw {\n\
                     private double radius;\n\
                     public Point center;\n\
                     public void draw() {}\n\
                   }\n\
                   class Point {}\n\
                   enum Color { RED, GREEN }\n";
        let out = generate_class_diagram(src, Some("java"), None).expect("java diagram");
        assert!(
            out.contains("<<interface>>") && out.contains("+draw() void"),
            "got:\n{out}"
        );
        assert!(out.contains("-radius: double"), "got:\n{out}");
        assert!(out.contains("+center: Point"), "got:\n{out}");
        assert!(out.contains("Shape <|-- Circle"), "got:\n{out}");
        assert!(out.contains("Draw <|.. Circle"), "got:\n{out}");
        assert!(out.contains("Circle *-- Point"), "got:\n{out}");
        assert!(
            out.contains("<<enum>>") && out.contains("RED"),
            "got:\n{out}"
        );
    }

    // --- C ---

    #[test]
    fn c_structs_typedef_enum_and_functions() {
        let src = "struct Point { double x; double y; };\n\
                   typedef struct { int a; } Pair;\n\
                   enum Color { RED, GREEN };\n\
                   double area(struct Point *p, int n) { return 0; }\n";
        let out = generate_class_diagram(src, Some("c"), None).expect("c diagram");
        assert!(
            out.contains("class Point {") && out.contains("+x: double"),
            "got:\n{out}"
        );
        assert!(
            out.contains("class Pair {") && out.contains("+a: int"),
            "got:\n{out}"
        );
        assert!(
            out.contains("<<enum>>") && out.contains("GREEN"),
            "got:\n{out}"
        );
        assert!(
            out.contains("<<module>>") && out.contains("+area("),
            "got:\n{out}"
        );
    }

    // --- C++ ---

    #[test]
    fn cpp_class_access_sections_and_inheritance() {
        let src = "class Circle : public Shape {\n\
                   public:\n    double radius;\n    void draw();\n    double area() const;\n\
                   private:\n    Point center;\n\
                   };\n\
                   struct Point { int x; };\n\
                   double area(Circle* c) { return 0; }\n";
        let out = generate_class_diagram(src, Some("cpp"), None).expect("cpp diagram");
        assert!(out.contains("+radius: double"), "got:\n{out}");
        assert!(out.contains("+draw() void"), "got:\n{out}");
        assert!(out.contains("-center: Point"), "got:\n{out}");
        assert!(out.contains("Shape <|-- Circle"), "got:\n{out}");
        assert!(out.contains("Circle *-- Point"), "got:\n{out}");
        assert!(
            out.contains("<<module>>") && out.contains("+area(Circle) double"),
            "got:\n{out}"
        );
    }

    // --- Ruby ---

    #[test]
    fn ruby_classes_methods_and_inheritance() {
        let src = "class Animal < Base\n  def speak(loud)\n  end\n  def self.make\n  end\nend\n\
                   def top_level\nend\n";
        let out = generate_class_diagram(src, Some("ruby"), None).expect("ruby diagram");
        assert!(out.contains("class Animal {"), "got:\n{out}");
        assert!(out.contains("+speak(loud)"), "got:\n{out}");
        assert!(out.contains("+self.make()"), "got:\n{out}");
        assert!(out.contains("Base <|-- Animal"), "got:\n{out}");
        assert!(
            out.contains("<<module>>") && out.contains("+top_level()"),
            "got:\n{out}"
        );
    }

    // --- PHP ---

    #[test]
    fn php_class_properties_methods_and_heritage() {
        let src = "<?php\n\
                   interface Draw { public function draw(): void; }\n\
                   class Circle extends Shape implements Draw {\n\
                     public float $radius;\n\
                     private Point $center;\n\
                     public function draw(): void {}\n\
                   }\n\
                   class Point {}\n\
                   function bounding(array $shapes): Point { return new Point(); }\n";
        let out = generate_class_diagram(src, Some("php"), None).expect("php diagram");
        assert!(
            out.contains("<<interface>>") && out.contains("+draw() void"),
            "got:\n{out}"
        );
        assert!(out.contains("+$radius: float"), "got:\n{out}");
        assert!(out.contains("-$center: Point"), "got:\n{out}");
        assert!(out.contains("Shape <|-- Circle"), "got:\n{out}");
        assert!(out.contains("Draw <|.. Circle"), "got:\n{out}");
        assert!(out.contains("Circle *-- Point"), "got:\n{out}");
        assert!(
            out.contains("<<module>>") && out.contains("+bounding(array) Point"),
            "got:\n{out}"
        );
    }
}
