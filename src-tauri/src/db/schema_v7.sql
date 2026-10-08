-- 迁移 user_version 6→7：批次21-B「图片原样收藏」。
-- 一列，只加不改（03 §6 规则）：media_path = 图片落在 app_data_dir/media 下的**相对文件名**
-- （`<fragment_id>.<ext>`）。存相对名而非绝对路径：整个应用数据目录搬家后图片仍然找得回来。
-- 为什么不复用既有列：
--   external_url 语义是"source=link 时的原链接"（03 §3.1），塞本地文件名会让该列说谎；
--   content 是人读的原文，图片没有原文——它只有一行占位标题（见 02 §1.3），
--   文件名必须由自己的列承载，purge 才知道该删哪个文件、导出才知道该写哪个相对路径。
ALTER TABLE fragments ADD COLUMN media_path TEXT;
