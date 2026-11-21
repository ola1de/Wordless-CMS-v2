-- Synthetic WordPress mysqldump for Wordless importer tests (Milestone 9).
-- Hand-written from the WordPress 7.1.5 schema; values are fictional.
-- phpass hashes below are fake (not real password hashes).

INSERT INTO `wp_users` VALUES
(1,'fixture-admin','$P$Bfakephpasshashfixtureadmin0000000000000000000','fixture-admin','admin@example.test','','2026-01-01 00:00:00','',0,'Fixture Admin'),
(2,'fixture-jane','$P$Bfakephpasshashfixturejane000000000000000000000','fixture-jane','jane@example.test','','2026-01-02 00:00:00','',0,'Jane Fixture');

INSERT INTO `wp_usermeta` VALUES
(1,1,'wp_capabilities','a:1:{s:13:"administrator";b:1;}'),
(2,1,'first_name','Fixture'),
(3,2,'wp_capabilities','a:1:{s:6:"author";b:1;}');

INSERT INTO `wp_terms` VALUES
(1,'News','news',0),
(2,'Tutorials','tutorials',0),
(3,'wordpress','wordpress',0),
(4,'migration','migration',0);

INSERT INTO `wp_term_taxonomy` VALUES
(1,1,'category','Fixture news.',0,1),
(2,2,'category','Fixture tutorials.',1,0),
(3,3,'post_tag','',0,1),
(4,4,'post_tag','',0,0);

INSERT INTO `wp_termmeta` VALUES
(1,1,'fixture_term_note','news term');

INSERT INTO `wp_posts` VALUES
(10,1,'2026-01-10 10:30:00','2026-01-10 10:30:00','<!-- wp:paragraph --><p>Hello, Wordless. This post came from WordPress.</p><!-- /wp:paragraph -->','Hello from WordPress','A fixture post.','publish','open','open','','hello-from-wordpress','2026-01-10 10:30:00','2026-01-10 10:30:00','',0,'https://example.test/?p=10',0,'post','',1),
(11,2,'2026-01-11 10:30:00','2026-01-11 10:30:00','<!-- wp:paragraph --><p>A fixture page.</p><!-- /wp:paragraph -->','About the Fixture','','publish','closed','closed','','about-the-fixture','2026-01-11 10:30:00','2026-01-11 10:30:00','',0,'https://example.test/?p=11',0,'page','',0),
(13,1,'2026-01-10 11:00:00','2026-01-10 11:00:00','Old revision content.','Hello from WordPress (revision)','','inherit','closed','closed','','10-revision-v1','2026-01-10 11:00:00','2026-01-10 11:00:00','',10,'https://example.test/?p=13',0,'revision','',0);

INSERT INTO `wp_postmeta` VALUES
(1,10,'_fixture_settings','a:2:{s:5:"color";s:4:"blue";s:4:"size";i:3;}'),
(2,10,'_thumbnail_id','12'),
(3,10,'_object_value','O:8:"stdClass":0:{}');

INSERT INTO `wp_term_relationships` VALUES
(10,1,0),
(10,3,0);

INSERT INTO `wp_comments` VALUES
(100,10,'Reader One','reader@example.test','','203.0.113.10','2026-01-11 09:00:00','2026-01-11 09:00:00','It\'s a fixture, and it works.',0,'1','','comment',0,0),
(101,10,'Reader Two','reader2@example.test','','203.0.113.11','2026-01-12 09:00:00','2026-01-12 09:00:00','Waiting for moderation.',0,'0','','comment',100,0);

INSERT INTO `wp_commentmeta` VALUES
(1,100,'fixture_comment_note','first comment');

INSERT INTO `wp_links` VALUES
(1,'https://example.test','Fixture Link','','','A fixture link.','Y',1,0,'2026-01-01 00:00:00','','','');

INSERT INTO `wp_options` VALUES
(1,'blogname','Fixture Site','yes'),
(2,'blogdescription','A synthetic WordPress export for Wordless importer tests.','yes'),
(3,'fixture_serialized_option','a:2:{s:5:"color";s:4:"blue";s:4:"size";i:3;}','yes'),
(4,'fixture_object_option','O:8:"stdClass":0:{}','yes');

-- A non-WordPress-core table: the importer must report it as ignored.
INSERT INTO `wp_custom_plugin` VALUES (1,'plugin-data');
