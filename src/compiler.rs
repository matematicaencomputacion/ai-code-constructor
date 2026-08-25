use std::fs;
use std::process::Command;

pub fn compile(code: &str) -> Result<(), String> {
    let file_path = "/tmp/generated.rs";

    // Guardamos el código generado en un archivo temporal.
    fs::write(file_path, code)
        .map_err(|error| format!("No se pudo escribir el archivo generado: {}", error))?;

    // Intentamos compilar el código Rust generado.
    let output = Command::new("rustc")
        .arg(file_path)
        .arg("--edition=2021")
        .arg("-o")
        .arg("/tmp/generated_program")
        .output()
        .map_err(|error| format!("No se pudo ejecutar rustc: {}", error))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}