-- Autenticação.
--
-- Até aqui existia UM usuário, resolvido no boot, e nenhuma barreira. Isso era
-- aceitável enquanto o servidor só existia dentro da tailnet — e virou o buraco
-- mais relevante no momento em que qualquer porta pudesse ser exposta.
--
-- Decisões:
--
--  * **Sessão opaca, não JWT.** JWT é stateless, o que soa bom até você querer
--    deslogar um aparelho perdido. Num servidor pessoal não há escala que
--    justifique abrir mão de revogação. A linha some da tabela e acabou.
--
--  * **O token não é guardado.** Guardamos o SHA-256 dele. Vazar o banco não
--    dá sessão a ninguém. Não precisa de Argon2 aqui: o token tem 256 bits de
--    entropia, não é adivinhável por força bruta como senha humana.
--
--  * **Senha com Argon2id**, que é resistente a GPU — ao contrário de bcrypt e
--    muito ao contrário de SHA.

ALTER TABLE app_user ADD COLUMN password_hash text;
ALTER TABLE app_user ADD COLUMN role text NOT NULL DEFAULT 'user'
    CHECK (role IN ('admin', 'user'));
ALTER TABLE app_user ADD COLUMN is_active boolean NOT NULL DEFAULT true;
ALTER TABLE app_user ADD COLUMN last_login_at timestamptz;

-- O usuário semeado no M0 vira admin, mas SEM senha: `password_hash IS NULL`
-- é o que sinaliza "primeira execução" e libera a rota de setup. Assim todo o
-- histórico de reprodução acumulado até aqui continua sendo dele.
UPDATE app_user SET role = 'admin';

CREATE TABLE auth_session (
    -- SHA-256 do token, em hex. O token em si nunca toca o disco.
    token_hash   text PRIMARY KEY,
    user_id      uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    -- Pra você reconhecer o aparelho na lista de sessões e revogar o certo.
    device_label text,
    user_agent   text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL
);

CREATE INDEX auth_session_user_idx    ON auth_session (user_id, last_seen_at DESC);
CREATE INDEX auth_session_expires_idx ON auth_session (expires_at);
