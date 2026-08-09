-- O nome pode mudar.
--
-- O `display_name` era escrito no nascimento da conta — no setup, na criação
-- pelo admin, no resgate de convite — e nunca mais. Não era decisão: era a
-- ausência de uma rota. O único `UPDATE ... display_name` do backend inteiro
-- estava no caminho de reivindicação da primeira execução, e ninguém tinha como
-- se renomear depois.
--
-- O limite de tamanho vive AQUI e não no Rust, pela regra que o `perfil.rs` já
-- segue com a bio e as tags: *"repeti-los aqui criaria dois lugares pra
-- discordar"*. O handler traduz a violação pra 400; o número mora num lugar só.
--
-- 40 caracteres: o nome aparece em linha de mural, em linha de placar e no
-- cabeçalho ao lado do anel. Mais que isso não cabe onde ele é lido.

-- Antes da trava, os dados. Uma migração que falha por causa de uma linha velha
-- derruba o boot do servidor, e o servidor não sobe pra explicar por quê — então
-- ela conserta o que a trava exigiria, na ordem em que precisa ser consertado.

-- 1. Espaço nas pontas e nomes longos demais. O `btrim` vem antes do corte pra
--    um nome que só tinha espaço na frente não perder letra à toa.
UPDATE app_user
   SET display_name = left(btrim(display_name), 40)
 WHERE display_name <> left(btrim(display_name), 40);

-- 2. O que sobrou vazio (era só espaço) volta pro username, que é o que o
--    `create_user` e o resgate de convite já usam como padrão. Nome vazio
--    deixaria um buraco em toda tela que mostra gente.
UPDATE app_user
   SET display_name = username
 WHERE btrim(display_name) = '';

ALTER TABLE app_user
  ADD CONSTRAINT app_user_display_name_check
  CHECK (length(btrim(display_name)) BETWEEN 1 AND 40);
