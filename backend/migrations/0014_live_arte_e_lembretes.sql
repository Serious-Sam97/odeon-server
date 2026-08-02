-- Arte nos cartões e lembretes de programa.

-- O XMLTV traz ano e categoria por programa; o modal do guia usa os dois, e o
-- ano é o que desempata título repetido na hora de achar a obra.
ALTER TABLE programme ADD COLUMN year int;
ALTER TABLE programme ADD COLUMN categoria text;

-- `work_id` já existia como gancho anulável desde o 0013. Agora ele é
-- preenchido — e o índice existe porque a tela busca a arte por programa.
CREATE INDEX programme_work_idx ON programme (work_id) WHERE work_id IS NOT NULL;

-- ------------------------------------------------------------- lembretes
--
-- "me avisa quando começar". Por usuário, porque o que eu quero ver não é o que
-- você quer ver — mesma razão de `playback_state` ser por usuário.
--
-- FK para `programme` com CASCADE: a grade é **regravada inteira** a cada
-- importação (ver `live::gravar_grade`), então todo lembrete morre junto. É
-- deliberado e é o comportamento correto: se o provedor reprogramou, o horário
-- que você agendou não existe mais. Guardar por título sobreviveria à
-- reimportação, mas passaria a avisar de uma reprise que você não pediu.
CREATE TABLE programme_reminder (
    user_id      uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    programme_id bigint NOT NULL REFERENCES programme(id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    -- Carimbado quando o aviso sai. Sem isto o mesmo lembrete dispararia a cada
    -- passada do vigia.
    notified_at  timestamptz,
    PRIMARY KEY (user_id, programme_id)
);

-- O vigia varre por "quem começa logo e ainda não avisei".
CREATE INDEX programme_reminder_pendente_idx
    ON programme_reminder (programme_id) WHERE notified_at IS NULL;
