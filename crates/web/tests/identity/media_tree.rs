use super::*;
use talocart_platform::media::library::{LibraryError, MediaLibrary};

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn shared_media_defaults_to_the_physical_root_and_keeps_nested_and_private_folders() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity, media.clone());
        let root = lib
            .create_folder(&s, &s.csrf, None, "Foto", false)
            .await
            .unwrap();
        let folder = lib.folder(&s, root, false).await.unwrap();
        assert_eq!(folder.path, "Foto");
        assert_eq!(folder.parent_id, None);
        assert!(media.root().join("Foto").is_dir());
        assert!(!media.root().join("library").exists());
        let child = lib
            .create_folder(&s, &s.csrf, Some(root), "Estate", false)
            .await
            .unwrap();
        assert_eq!(
            lib.folder(&s, child, false).await.unwrap().path,
            "Foto/Estate"
        );
        lib.move_folder(&s, &s.csrf, child, None, "Estate", false)
            .await
            .unwrap();
        let child = lib.folder(&s, child, false).await.unwrap();
        assert_eq!(child.path, "Estate");
        assert_eq!(child.parent_id, None);
        let id = lib
            .upload(&s, &s.csrf, None, b"root file".to_vec(), "root.txt", false)
            .await
            .unwrap();
        let asset = lib.asset(&s, id, false).await.unwrap();
        assert_eq!(asset.path, "");
        assert_eq!(
            tokio::fs::read(media.root().join("root.txt"))
                .await
                .unwrap(),
            b"root file"
        );
        assert!(!media.root().join("library").exists());
        let private = lib
            .create_folder(&s, &s.csrf, None, "Personale", true)
            .await
            .unwrap();
        assert_eq!(
            lib.folder(&s, private, true).await.unwrap().path,
            format!("private/account-{}/Personale", s.administrator_id.unwrap())
        );
        for name in ["cache", "staging", "private", "objects", "brand", "LIBRARY"] {
            assert!(
                lib.create_folder(&s, &s.csrf, None, name, false)
                    .await
                    .is_err(),
                "Reserved root: {name}"
            );
        }
        let tree = lib.folder_tree(&s, None, 1, false).await.unwrap();
        assert!(tree[0].entries.entries.iter().any(|f| f.id == root));
        // A real legacy library stays usable; changing defaults never moves its contents.
        let legacy: i64 = sqlx::query_scalar("SELECT id FROM media_folders WHERE path='library'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let old = lib
            .create_folder(&s, &s.csrf, Some(legacy), "Archivio", false)
            .await
            .unwrap();
        assert_eq!(
            lib.folder(&s, old, false).await.unwrap().path,
            "library/Archivio"
        );
        lib.move_folder(&s, &s.csrf, old, None, "Archivio", false)
            .await
            .unwrap();
        assert_eq!(lib.folder(&s, old, false).await.unwrap().path, "Archivio");
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn folder_tree_bounds_each_branch_and_keeps_the_selected_ancestry_visible() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity, media.clone());
        let initial_roots: i64 = 0;
        sqlx::query("INSERT INTO media_folders(name,path) SELECT 'Root '||lpad(n::text,3,'0'),'root-'||n FROM generate_series(1,105)n")
            .execute(&pool).await.unwrap();
        let root: i64 = sqlx::query_scalar("SELECT id FROM media_folders WHERE path='root-105'").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO media_folders(parent_id,name,path) SELECT $1,'Child '||lpad(n::text,3,'0'),'root-105/child-'||n FROM generate_series(1,107)n")
            .bind(root).execute(&pool).await.unwrap();
        let child: i64 = sqlx::query_scalar("SELECT id FROM media_folders WHERE path='root-105/child-107'").fetch_one(&pool).await.unwrap();
        let leaf: i64 = sqlx::query_scalar("INSERT INTO media_folders(parent_id,name,path) VALUES($1,'Leaf <test>','root-105/child-107/leaf') RETURNING id")
            .bind(child).fetch_one(&pool).await.unwrap();
        let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM media_folders WHERE path LIKE 'root-%'").fetch_all(&pool).await.unwrap();
        for path in paths { tokio::fs::create_dir_all(media.root().join(path)).await.unwrap(); }
        let branches = lib.folder_tree(&s, Some(leaf), 1, false).await.unwrap();
        assert_eq!(branches.len(), 4);
        assert!(branches.iter().all(|b| b.entries.entries.len() <= 100));
        let roots = branches.iter().find(|b| b.parent.is_none()).unwrap();
        assert_eq!((roots.entries.total, roots.entries.page, roots.entries.entries.len()), (105 + initial_roots, 2, usize::try_from(5 + initial_roots).unwrap()));
        assert!(roots.entries.entries.iter().any(|f| f.id == root && f.has_children));
        let children = branches.iter().find(|b| b.parent == Some(root)).unwrap();
        assert_eq!((children.entries.total, children.entries.page, children.entries.entries.len()), (107, 2, 7));
        assert!(children.entries.entries.iter().any(|f| f.id == child && f.has_children));
        let page = lib.folder_tree(&s, Some(root), 999, false).await.unwrap();
        let children = page.iter().find(|b| b.parent == Some(root)).unwrap();
        assert_eq!((children.entries.page, children.entries.entries.len()), (2, 7));
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool, "Tree".into()).await);
        let response = request(&app, "GET", &format!("/admin/media?folder={leaf}&q=needle&format=pdf&usage=unused&sort=newest"), &cookie, "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = html(response).await;
        assert!(body.contains(&format!("data-folder=\"{root}\" open")));
        assert!(body.contains(&format!("data-folder=\"{child}\" open")));
        assert!(body.contains("Leaf &#60;test&#62;</span>"));
        assert!(body.contains("Cartelle 101–107 di 107"));
        assert!(body.contains("q=needle&#38;format=pdf&#38;usage=unused&#38;sort=newest"));
        let picker = html(request(&app, "GET", &format!("/admin/media?folder={leaf}&target=gallery%3A42"), &cookie, "", false).await).await;
        assert!(picker.contains("target=gallery%3A42"));
        assert!(picker.contains("Tutte le immagini") && !picker.contains("Tutti i file"));
        let response = request(&app, "GET", &format!("/admin/media/tree/{root}?folders_page=99"), &cookie, "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let branch = html(response).await;
        assert!(branch.contains("Child 107") && !branch.contains("Child 001"));
        assert!(!branch.contains("<!DOCTYPE") && !branch.contains("Root 101"));
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn folder_tree_and_lazy_branches_preserve_private_ownership_and_authorization() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let lib = MediaLibrary::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let shared = lib
            .create_folder(&s, &s.csrf, None, "Shared tree", false)
            .await
            .unwrap();
        let private = lib
            .create_folder(&s, &s.csrf, None, "Private tree", true)
            .await
            .unwrap();
        assert!(matches!(
            lib.folder_tree(&s, Some(private), 1, false).await,
            Err(LibraryError::NotFound)
        ));
        assert!(lib.folder_tree(&s, Some(private), 1, true).await.is_ok());
        let role = identity
            .create_role(&s, &s.csrf, "Tree account", &[])
            .await
            .unwrap();
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "tree-other@example.test",
                "correct horse battery".into(),
                talocart_platform::identity::Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let other = login_as(&identity, "tree-other@example.test").await;
        let app = router(test_app(pool.clone(), "Tree".into()).await);
        let cookie = format!("talocart_session={raw}");
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let body = html(
                request(
                    &app,
                    "GET",
                    &format!("/admin/media?folder={shared}"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(body.contains(if locale == "it" {
                "Navigazione cartelle"
            } else {
                "Folder navigation"
            }));
            assert!(!body.contains("Private tree"));
            let body = html(
                request(
                    &app,
                    "GET",
                    &format!("/admin/media?folder={private}&personal=true"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(body.contains("Private tree") && !body.contains("Shared tree"));
        }
        for (route, who, expected) in [
            (
                format!("/admin/media/tree/{private}"),
                cookie.as_str(),
                StatusCode::NOT_FOUND,
            ),
            (
                format!("/admin/media/tree/{private}?personal=true"),
                "",
                StatusCode::SEE_OTHER,
            ),
            (
                format!("/admin/media/tree/{private}?personal=true"),
                &format!("talocart_session={other}"),
                StatusCode::FORBIDDEN,
            ),
            (
                format!("/admin/media/tree/{private}?personal=true&target=avatar"),
                &format!("talocart_session={other}"),
                StatusCode::NOT_FOUND,
            ),
            (
                format!("/admin/media/tree/{shared}"),
                &format!("talocart_session={other}"),
                StatusCode::FORBIDDEN,
            ),
            (
                format!("/admin/media/tree/{shared}?folders_page=-1"),
                cookie.as_str(),
                StatusCode::OK,
            ),
            (
                format!("/admin/media/tree/{shared}?folders_page=1000001"),
                cookie.as_str(),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/admin/media/tree/9223372036854775807".into(),
                cookie.as_str(),
                StatusCode::NOT_FOUND,
            ),
        ] {
            assert_eq!(
                request(&app, "GET", &route, who, "", false).await.status(),
                expected,
                "{route}"
            );
        }
    })
    .await;
}
