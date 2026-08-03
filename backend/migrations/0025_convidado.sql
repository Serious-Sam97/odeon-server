-- R26 — o convidado.
--
-- ## A auditoria que motivou esta migração
--
-- O `IDEIAS.md` §6.5 adiou "gente de fora" e deixou escrito que três decisões
-- do projeto tinham sido tomadas assumindo uma tailnet de aparelhos seus.
-- Medido agora, com uma conta comum de verdade, o buraco é maior que os três
-- itens da lista:
--
-- | o que uma conta `user` alcança | rota |
-- |---|---|
-- | os caminhos do seu disco | `/api/libraries` → `/media/Movies` |
-- | o layout das montagens | `/api/storage` → `gravavel: true` |
-- | quem está assistindo o quê, agora | `/api/transcode/sessions` |
-- | **o acervo inteiro, sem escopo** | `/api/stream/{qualquer}` → **206** |
--
-- ## O papel novo, e por que `user` não servia
--
-- Hoje há `admin` e `user`, e `user` significa **morador**: alguém cujo disco
-- aquele é. Um convidado não é isso, e chamá-lo de `user` é o que produziria
-- os quatro achados acima em silêncio.
--
-- `guest` é o papel de quem foi convidado pra um círculo e **não é dono de
-- nada**. Ele navega o acervo — uma locadora deixa ler a caixa toda — mas o que
-- ele pode **assistir** é outra pergunta, e a resposta estava pronta desde a
-- R19.
--
-- ## O empréstimo deixa de ser teatro
--
-- A R19 (§35) decidiu que o bloqueio vale na locadora e não no player, e isso
-- estava **certo** para a casa: o disco é seu, e barrar o player transformaria
-- um morador em porteiro do outro.
--
-- Para um convidado a mesma regra é exatamente errada — entrar no círculo
-- entregaria a prateleira inteira, e a escassez viraria encenação. Então para
-- ele a regra se inverte, e é a inversão que dá sentido a tudo que a R19 e a
-- R20 construíram:
--
-- > **o convidado só assiste o que pegou emprestado.**
--
-- Uma cópia por caixa (§35) passa a ser verdade técnica, e não só social; o
-- prazo passa a ser quando o acesso termina; a devolução automática passa a ser
-- a revogação. Nada disso precisou ser inventado agora — só deixou de valer
-- apenas para o morador.

ALTER TABLE app_user DROP CONSTRAINT IF EXISTS app_user_role_check;
ALTER TABLE app_user ADD CONSTRAINT app_user_role_check
    CHECK (role IN ('admin', 'user', 'guest'));

COMMENT ON COLUMN app_user.role IS
    'admin: dono · user: morador (o disco é dele também) · guest: convidado, só assiste o que pegou emprestado';

-- ------------------------------------------------------------- o convite
--
-- Um convidado não se cria sozinho e não se cria por senha combinada por fora:
-- ele nasce de um convite que alguém **do círculo** emitiu.
--
-- O token não é guardado — guardamos o SHA-256, exatamente como `auth_session`
-- faz desde o §9b. Vazar o banco não dá convite a ninguém.
CREATE TABLE convite (
    -- SHA-256 do código, em hex. O código em si nunca toca o disco.
    codigo_hash  text PRIMARY KEY,

    circulo_id   uuid NOT NULL REFERENCES circulo(id) ON DELETE CASCADE,
    criado_por   uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    criado_em    timestamptz NOT NULL DEFAULT now(),

    -- Pra quem, na intenção de quem convidou. É rótulo, não identidade — serve
    -- pra lista de convites dizer "pro Rudney" em vez de mostrar um hash.
    para         text,

    -- **Convite vence.** Um convite eterno é uma senha permanente esquecida num
    -- aplicativo de mensagem. Sete dias é o mesmo prazo da fita (§35), e a
    -- coincidência é de propósito: as duas coisas são empréstimos de acesso.
    expira_em    timestamptz NOT NULL,

    -- Usado uma vez e só. `usado_por` fica pra que a lista de convites diga
    -- quem entrou por qual — auditoria mínima, e a única que faz sentido num
    -- servidor de uma pessoa.
    usado_em     timestamptz,
    usado_por    uuid REFERENCES app_user(id) ON DELETE SET NULL,

    CONSTRAINT convite_uso_completo CHECK (
        (usado_em IS NULL AND usado_por IS NULL)
     OR (usado_em IS NOT NULL AND usado_por IS NOT NULL)
    )
);

-- "Os convites deste círculo, os abertos primeiro" — a pergunta da tela de
-- administração.
CREATE INDEX convite_circulo_idx ON convite (circulo_id, criado_em DESC);

-- A varredura de limpeza: convite vencido e não usado não serve pra nada.
CREATE INDEX convite_vencendo_idx ON convite (expira_em) WHERE usado_em IS NULL;
