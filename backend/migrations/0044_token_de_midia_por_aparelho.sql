-- O token de mídia passa a ser **por aparelho**, e não por conta (R61).
--
-- ## A pergunta que o cliente fez
--
-- > *"O token de mídia é por conta ou por aparelho? Se for por aparelho, o
-- > problema some sozinho."*
--
-- Era por conta, e o `emitir_token_de_midia` apagava os anteriores do usuário
-- antes de gravar o novo. Com celular e TV abertos ao mesmo tempo, quem pedia
-- o dele derrubava o do outro no meio da reprodução. Os clientes já haviam
-- posto uma trava pra renovar **uma** vez só, o que reduz o sintoma e não toca
-- a causa: dois aparelhos legítimos continuam sendo dois.
--
-- ## A resposta
--
-- Por aparelho. Um token de mídia existe pra **um** player buscar bytes; dois
-- players são dois fatos independentes, e a conta não é o que os une.
--
-- A decisão não é nova, é a mesma da R45 vista de outro ângulo: lá o token de
-- **arte** ganhou teto em vez de purga, porque o app da TV republicava a
-- fileira e apagava a credencial que ele mesmo tinha acabado de publicar. O
-- token de mídia sofria do mesmo erro, só que entre aparelhos em vez de dentro
-- de um.
--
-- O que ele **não** vira é ilimitado: a poda continua existindo, só que a chave
-- dela deixa de ser a conta e passa a ser a sessão. Renovar do mesmo aparelho
-- aposenta o token daquele aparelho, como antes; do aparelho do lado, não
-- encosta. E o número de sessões vivas é o número de aparelhos que alguém
-- realmente pareou — um teto que a própria casa impõe.
--
-- `ON DELETE CASCADE` de propósito: sair da conta num aparelho tem de levar a
-- credencial de mídia dele junto. Era o único jeito de revogar antes — e era
-- por acidente, porque revogava a de todo mundo.
ALTER TABLE media_token
    ADD COLUMN sessao_id uuid REFERENCES auth_session(id) ON DELETE CASCADE;

-- Os que já existem não têm dono conhecido; ficam `NULL` e serão aposentados
-- pela primeira renovação de quem não tiver sessão identificada, ou pelo
-- vencimento. Não vale chutar uma sessão pra eles.
CREATE INDEX media_token_sessao_idx
    ON media_token (user_id, sessao_id)
 WHERE escopo = 'midia';
