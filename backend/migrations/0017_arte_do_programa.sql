-- O programa guarda a própria arte.
--
-- Até aqui um programa só tinha capa se o título casasse com uma obra da
-- biblioteca — 233 de 926 neste acervo, 25%. Mas o XMLTV **já mandava a
-- imagem** em 827 dos 919 programas, e o importador a jogava fora.
--
-- `arte` é o caminho relativo dentro do diretório de artwork do Odeon, igual
-- ao de `work.artwork`: a imagem é baixada e servida pelo próprio servidor.
-- Apontar o navegador direto pro ErsatzTV não serviria — a URL dele é um
-- endereço da bridge do Docker, que só existe nesta máquina, e o cliente do
-- Mac na tailnet veria buraco.
--
-- `arte_url` fica junto pra a reimportação saber que aquela imagem já foi
-- baixada e não pedir de novo.
ALTER TABLE programme
    ADD COLUMN IF NOT EXISTS arte text,
    ADD COLUMN IF NOT EXISTS arte_url text;

-- A grade é substituída inteira a cada importação; o cache de download é por
-- URL, e é este índice que faz a consulta "já baixei esta?" não varrer tudo.
CREATE INDEX IF NOT EXISTS programme_arte_url_idx
    ON programme (arte_url) WHERE arte_url IS NOT NULL;
