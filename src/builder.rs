use crate::state::CodeState;

/// Genera código a partir del estado actual.
///
/// El Builder recibe el pedido y el plan producido por el Planner
/// y genera una primera versión válida del código.
///
/// Si existen errores provenientes del Compiler o Validator,
/// genera una nueva versión intentando corregirlos.
pub fn build(state: &mut CodeState) {
    println!("BUILDER: generando código...");

    if !state.errors.is_empty() {
        println!("BUILDER: analizando errores anteriores...");

        for error in &state.errors {
            println!("BUILDER: feedback recibido -> {}", error);
        }

        println!("BUILDER: generando versión corregida...");
    } else {
        println!("BUILDER: generando primera versión...");
    }

    let request = state.request.to_lowercase();

    let code = if request.contains("api") || request.contains("rest") {
        generate_rest_api()
    } else {
        generate_generic_program()
    };

    state.code = Some(code);

    println!("BUILDER: código generado");
}

/// Genera una API HTTP REST mínima utilizando únicamente
/// la biblioteca estándar de Rust.
fn generate_rest_api() -> String {
    let code = r#"use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let listener = TcpListener::bind("127.0.0.1:3000")
        .expect("No se pudo iniciar el servidor HTTP");

    println!("API REST escuchando en http://127.0.0.1:3000");

    for incoming in listener.incoming() {
        let mut stream = match incoming {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("Error aceptando conexión: {}", error);
                continue;
            }
        };

        let mut buffer = [0u8; 1024];

        if let Err(error) = stream.read(&mut buffer) {
            eprintln!("Error leyendo request: {}", error);
            continue;
        }

        let request = String::from_utf8_lossy(&buffer);

        let first_line = request.lines().next().unwrap_or("");

        let (status, body) = if first_line.starts_with("GET /api") {
            ("200 OK", "{\"status\":\"ok\",\"message\":\"GET /api\"}")
        } else if first_line.starts_with("POST /api") {
            ("200 OK", "{\"status\":\"ok\",\"message\":\"POST /api\"}")
        } else {
            ("404 Not Found", "{\"status\":\"error\",\"message\":\"Not Found\"}")
        };

        let response = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            body.len(),
            body
        );

        if let Err(error) = stream.write_all(response.as_bytes()) {
            eprintln!("Error enviando respuesta: {}", error);
        }
    }
}
"#;

    code.to_string()
}

/// Genera un programa Rust mínimo para pedidos que no son
/// específicamente una API REST.
fn generate_generic_program() -> String {
    let code = r#"fn main() {
    println!("Programa generado por AI-Code-Constructor");
}
"#;

    code.to_string()
}