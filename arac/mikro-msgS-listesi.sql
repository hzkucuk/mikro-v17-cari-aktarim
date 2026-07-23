/* ============================================================================
   Mikro msg_S_XXXX alias karşılıkları — tam liste üretici
   ----------------------------------------------------------------------------
   Mikro view/CHOOSE kolonlarındaki "msg_S_0888" gibi alias'lar, dil kaynak
   sisteminden çözülür:
     - Tablo   : dbo.mye_LANGUAGE_RESOURCES
     - Fonksiyon: dbo.fn_GetResource('S', '<4 haneli kod>', '')
     - Dosya    : mye_LANGUAGE_RESOURCES.TXT  (satır = 10190 + kod numarası)
   İlk karakter dil: 1=Türkçe, 2=İngilizce, 3=Almanca.

   Bu betiği, fn_GetResource fonksiyonunun bulunduğu Mikro veritabanında
   (örn. MikroDesktop_BEDIR_2017) çalıştırın.

   TXT olarak almak için SSMS'te:
     Query > Results To > Results to File   (veya sqlcmd -o cikti.txt)
   ============================================================================ */

SET NOCOUNT ON;

/* ----------------------------------------------------------------------------
   YÖNTEM 1 (önerilen, sağlam): 0000-9999 arası tüm S kodlarını fonksiyonla
   çöz, boş olmayanları listele. Tablo yapısından bağımsız çalışır.
   ---------------------------------------------------------------------------- */
;WITH sayilar AS (
    SELECT TOP (10000)
           ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) - 1 AS num
    FROM sys.all_objects a CROSS JOIN sys.all_objects b
)
SELECT
    'msg_S_' + RIGHT('0000' + CAST(num AS varchar(5)), 4)              AS alias,
    RIGHT('0000' + CAST(num AS varchar(5)), 4)                         AS kod,
    dbo.fn_GetResource('S', RIGHT('0000' + CAST(num AS varchar(5)), 4), '') AS turkce_karsilik
FROM sayilar
WHERE dbo.fn_GetResource('S', RIGHT('0000' + CAST(num AS varchar(5)), 4), '') <> ''
ORDER BY num;

/* ----------------------------------------------------------------------------
   YÖNTEM 2 (alternatif): doğrudan kaynak tablosundan.
   mye_LANGUAGE_RESOURCES kolon adları sürümlere göre değişebildiğinden, önce
   yapıyı görmek için şunu çalıştırın, sonra kolon adlarını aşağıda güncelleyin:

       SELECT TOP 20 * FROM dbo.mye_LANGUAGE_RESOURCES;
       SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS
       WHERE TABLE_NAME = 'mye_LANGUAGE_RESOURCES' ORDER BY ORDINAL_POSITION;

   Tipik kullanım (kolon adları kuruluma göre uyarlanmalı):

       SELECT 'msg_S_' + res_kodu AS alias, res_metni AS turkce
       FROM dbo.mye_LANGUAGE_RESOURCES
       WHERE res_tipi = 'S'
       ORDER BY res_kodu;
   ---------------------------------------------------------------------------- */

/* Tek bir kodu hızlı sorgulamak için:
       SELECT dbo.fn_GetResource('S', '0888', '');   -- -> HAREKET TİPİ
*/
