use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COMPILE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Compila `code` con `rustc` usando archivos temporales únicos por invocación.
///
/// Evita carreras entre tests/procesos concurrentes que antes compartían
/// `/tmp/generated.rs` y podían mezclar fragmentos de distintos planes.
pub fn compile(code: &str) -> Result<(), String> {
    let seq = COMPILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_path = format!("/tmp/ai_code_constructor_{}_{}.rs", std::process::id(), seq);
    let output_path = format!(
        "/tmp/ai_code_constructor_{}_{}.bin",
        std::process::id(),
        seq
    );

    let write_result = fs::write(&file_path, code)
        .map_err(|error| format!("No se pudo escribir el archivo generado: {}", error));

    let compile_result = write_result.and_then(|_| {
        let output = Command::new("rustc")
            .arg(&file_path)
            .arg("--edition=2021")
            .arg("-o")
            .arg(&output_path)
            .output()
            .map_err(|error| format!("No se pudo ejecutar rustc: {}", error))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    });

    let _ = fs::remove_file(&file_path);
    let _ = fs::remove_file(&output_path);

    compile_result
}
