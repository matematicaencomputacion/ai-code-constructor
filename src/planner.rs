use crate::state::CodeState;

pub fn plan(state: &mut CodeState) {
    state.plan = Some(
        "1. Crear servidor HTTP\n\
2. Definir endpoints\n\
3. Implementar handlers\n\
4. Agregar tests"
            .to_string(),
    );


}