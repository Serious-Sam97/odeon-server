-- R46 — o pareamento: entrar na TV pelo celular.
--
-- ## O que custa hoje
--
-- Digitar numa TV é soletrar com o D-pad. Uma senha de doze caracteres custa
-- uns oitenta apertos, e o teclado da Google TV esconde metade da tela
-- enquanto isso. Nada quebra — é a primeira tela do produto sendo a pior, e é
-- por isso que todo serviço grande de TV empurra o login pro celular.
--
-- ## O sentido do fluxo, que não é o do convite
--
-- O celular **já logado** pede um código; a TV troca o código por sessão. Quem
-- prova quem é, portanto, é o celular — a TV só carrega a prova. É o mesmo
-- desenho do `convite` uma camada acima: `POST /api/convites` exige sessão e
-- `POST /api/convites/resgatar` é público, porque a troca acontece **antes** de
-- haver sessão do outro lado.
--
-- ## Por que aqui um código curto é aceitável, e no convite não era
--
-- O `convite` cravou 128 bits com uma frase que continua certa: *"um código
-- curto seria adivinhável, e este código **é** a autenticação de quem
-- resgata"*. Vale igual aqui — o código É a autenticação. O que muda são os
-- números em volta dele, e são três:
--
--   * **Cinco minutos**, e não sete dias. O código nasce pra atravessar a sala.
--   * **Um uso.** O `usado_em` fecha na mesma transação que resolve o resgate.
--   * **Um código vivo por pessoa.** Pedir um novo mata o anterior, então o
--     alvo não cresce com o tempo — ele é do tamanho da casa.
--
-- Com 40 bits e no máximo um punhado de códigos vivos, mil tentativas por
-- segundo durante a janela inteira dão ordem de 10^-6 de acerto. O convite não
-- podia se dar a isso porque vale uma semana e **cria conta**; este vale
-- minutos e devolve sessão de uma conta que já existe.
--
-- Guardamos o SHA-256, como `auth_session`, `convite` e `media_token`. Vazar o
-- banco não dá pareamento a ninguém — e, com cinco minutos de validade, nem
-- daria tempo.

CREATE TABLE pareamento (
    -- SHA-256 do código normalizado (maiúsculas, sem separador). O código em si
    -- nunca toca o disco.
    codigo_hash text PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    criado_em   timestamptz NOT NULL DEFAULT now(),
    expira_em   timestamptz NOT NULL,
    -- Quando a TV o trocou por sessão. Não-nulo = queimado, e o resgate confere
    -- isso na mesma transação em que grava — é o que impede dois aparelhos de
    -- entrarem com o mesmo código.
    usado_em    timestamptz
);

-- "O código vivo desta pessoa" — pedir um novo apaga o anterior, e é essa
-- consulta que faz a poda. O índice parcial cobre só o que ainda serve.
CREATE INDEX pareamento_vivo_idx
    ON pareamento (user_id)
    WHERE usado_em IS NULL;
