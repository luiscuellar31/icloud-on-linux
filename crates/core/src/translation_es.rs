//! Spanish text for the app. Each entry pairs the English source text passed to `tr!` with its translation.

pub static SPANISH: &[(&str, &str)] = &[
    // Desktop app
    ("Menu", "Menú"),
    ("Not signed in", "Sin sesión iniciada"),
    ("Disconnected", "Desconectado"),
    ("Connecting...", "Conectando..."),
    ("Connected", "Conectado"),
    ("Disconnecting...", "Desconectando..."),
    ("Reconnecting...", "Reconectando..."),
    ("Needs attention", "Requiere atención"),
    ("Review needed", "Revisión pendiente"),
    ("Sync problem", "Problema de sincronización"),
    ("Upload problem", "Problema con las subidas"),
    ("Uploading...", "Subiendo..."),
    ("Welcome", "Te damos la bienvenida"),
    (
        "Sign in with your Apple account to open iCloud Drive in your file manager and keep folders in sync on this computer.",
        "Inicia sesión con tu cuenta de Apple para abrir iCloud Drive en tu gestor de archivos y mantener carpetas sincronizadas en este equipo.",
    ),
    (
        "The key you chose when signing in. It is not your Apple password.",
        "La clave que elegiste al iniciar sesión. No es tu contraseña de Apple.",
    ),
    ("Connect", "Conectar"),
    ("Open iCloud Drive", "Abrir iCloud Drive"),
    ("Add folder", "Agregar carpeta"),
    ("Upload files...", "Subir archivos..."),
    ("Renew Apple session...", "Renovar sesión de Apple..."),
    ("Renew Apple session", "Renovar sesión de Apple"),
    (
        "Renewing disconnects iCloud Drive and pauses syncing until you connect again. Continue?",
        "Para renovar la sesión se desconecta iCloud Drive y la sincronización queda en pausa hasta que vuelvas a conectarte. ¿Continuar?",
    ),
    ("Language", "Idioma"),
    ("Automatic", "Automático"),
    ("Disconnect", "Desconectar"),
    ("Quit", "Salir"),
    (
        "Signed in. Enter your local key to connect.",
        "Sesión iniciada. Ingresa tu clave local para conectarte.",
    ),
    (
        "The file manager could not be opened. You can still disconnect safely.",
        "No se pudo abrir el gestor de archivos. Aun así puedes desconectarte de forma segura.",
    ),
    // Background mode
    ("Start at login", "Abrir al iniciar sesión"),
    (
        "Unlock automatically on this computer",
        "Desbloquear automáticamente en este equipo",
    ),
    (
        "Saves the local key in your desktop keyring, so your Linux login protects your iCloud Drive instead.",
        "Guarda la clave local en el llavero del escritorio; así tu inicio de sesión de Linux protege tu iCloud Drive.",
    ),
    (
        "Install secret-tool (libsecret-tools) to save the key in your desktop keyring.",
        "Instala secret-tool (libsecret-tools) para guardar la clave en el llavero del escritorio.",
    ),
    ("Unlock automatically", "Desbloquear automáticamente"),
    (
        "Your local key will be saved in the desktop keyring. Anyone who can use your Linux account will be able to open your iCloud Drive without typing it. Turn this on?",
        "Tu clave local se guardará en el llavero del escritorio. Cualquiera que pueda usar tu cuenta de Linux podrá abrir tu iCloud Drive sin escribirla. ¿Activar esta opción?",
    ),
    (
        "The key could not be saved in the desktop keyring. Type it the next time you connect.",
        "No se pudo guardar la clave en el llavero del escritorio. Escríbela la próxima vez que te conectes.",
    ),
    ("Renew", "Renovar"),
    (
        "Your Apple session may have ended. Renew it to keep syncing.",
        "Es posible que tu sesión de Apple haya terminado. Renuévala para seguir sincronizando.",
    ),
    (
        "Your Apple session ends in less than 2 days. Renew it to keep syncing.",
        "Tu sesión de Apple termina en menos de 2 días. Renuévala para seguir sincronizando.",
    ),
    (
        "Your Apple session ends in about {days} days. Renew it to keep syncing.",
        "Tu sesión de Apple termina en unos {days} días. Renuévala para seguir sincronizando.",
    ),
    ("Renew your Apple session", "Renueva tu sesión de Apple"),
    ("iCloud Drive needs attention", "iCloud Drive requiere atención"),
    (
        "A synced folder needs your review",
        "Una carpeta sincronizada requiere tu revisión",
    ),
    ("Uploads need your decision", "Las subidas requieren tu decisión"),
    ("Show window", "Mostrar ventana"),
    ("iCloud on Linux is still running", "iCloud on Linux sigue en ejecución"),
    (
        "Syncing continues in the background. Use its icon to open the window or quit.",
        "La sincronización continúa en segundo plano. Usa su icono para abrir la ventana o salir.",
    ),
    ("Keep syncing?", "¿Seguir sincronizando?"),
    (
        "iCloud on Linux can keep syncing after you close this window. To see it again, open iCloud on Linux from your applications.",
        "iCloud on Linux puede seguir sincronizando después de cerrar esta ventana. Para volver a verla, abre iCloud on Linux desde tus aplicaciones.",
    ),
    ("Keep syncing", "Seguir sincronizando"),
    ("Quit iCloud on Linux", "Salir de iCloud on Linux"),
    (
        "Quitting pauses uploads, stops syncing, and disconnects iCloud Drive. Quit now?",
        "Al salir se pausan las subidas, se detiene la sincronización y se desconecta iCloud Drive. ¿Salir ahora?",
    ),
    // Sign-in
    ("Sign in to iCloud", "Iniciar sesión en iCloud"),
    (
        "Use your Apple account password and verification code. Your local key encrypts the saved session.",
        "Usa la contraseña de tu cuenta de Apple y el código de verificación. Tu clave local cifra la sesión guardada.",
    ),
    ("Local key", "Clave local"),
    ("Confirm new local key", "Confirma la clave local nueva"),
    ("Apple account", "Cuenta de Apple"),
    ("Apple password", "Contraseña de Apple"),
    ("6-digit verification code", "Código de verificación de 6 dígitos"),
    ("Trusted phone number", "Número de teléfono de confianza"),
    ("Sign in", "Iniciar sesión"),
    ("Send SMS", "Enviar SMS"),
    ("Continue", "Continuar"),
    ("Cancel", "Cancelar"),
    ("Close", "Cerrar"),
    (
        "Enter your local key, Apple account, and Apple password.",
        "Ingresa tu clave local, tu cuenta de Apple y tu contraseña de Apple.",
    ),
    (
        "The new local keys do not match.",
        "Las claves locales nuevas no coinciden.",
    ),
    (
        "Install rclone and the desktop app before signing in.",
        "Instala rclone y la aplicación de escritorio antes de iniciar sesión.",
    ),
    (
        "The saved profile or its private folder could not be opened.",
        "No se pudo abrir el perfil guardado o su carpeta privada.",
    ),
    ("Checking access...", "Verificando el acceso..."),
    ("Signing in to iCloud...", "Iniciando sesión en iCloud..."),
    (
        "Sign-in cancelled. Your saved session was preserved.",
        "Inicio de sesión cancelado. Tu sesión guardada se conservó.",
    ),
    (
        "Sign-in failed. Check your local key, Apple credentials, verification code, and connection, then try again.",
        "No se pudo iniciar sesión. Revisa tu clave local, tus datos de Apple, el código de verificación y tu conexión, y vuelve a intentarlo.",
    ),
    (
        "Use the same Apple account to renew this profile. Pending uploads were preserved.",
        "Usa la misma cuenta de Apple para renovar este perfil. Las subidas pendientes se conservaron.",
    ),
    (
        "Choose a trusted phone number.",
        "Elige un número de teléfono de confianza.",
    ),
    (
        "Choose a listed trusted phone number.",
        "Elige uno de los números de teléfono de confianza de la lista.",
    ),
    (
        "Enter the verification code from Apple.",
        "Ingresa el código de verificación de Apple.",
    ),
    (
        "Enter the 6-digit verification code.",
        "Ingresa el código de verificación de 6 dígitos.",
    ),
    (
        "The verification response was not accepted. Try again.",
        "No se aceptó la respuesta de verificación. Vuelve a intentarlo.",
    ),
    (
        "The saved profile may have been updated. Reconnect to check it; pending copies are retained.",
        "Es posible que el perfil guardado se haya actualizado. Vuelve a conectarte para comprobarlo; las copias pendientes se conservan.",
    ),
    (
        "The sign-in response or saved profile could not be verified. Your saved session was preserved.",
        "No se pudo verificar la respuesta del inicio de sesión o el perfil guardado. Tu sesión guardada se conservó.",
    ),
    (
        "Temporary encrypted sign-in files could not be removed.",
        "No se pudieron borrar los archivos cifrados temporales del inicio de sesión.",
    ),
    (
        "Signed in. Your encrypted session is ready. Connect with your local key.",
        "Sesión iniciada. Tu sesión cifrada está lista. Conéctate con tu clave local.",
    ),
    // Saved profile
    (
        "A private application folder is unavailable.",
        "Una carpeta privada de la aplicación no está disponible.",
    ),
    (
        "The saved session must be a private file owned by your user.",
        "La sesión guardada debe ser un archivo privado de tu usuario.",
    ),
    (
        "The saved session must be encrypted before using this app.",
        "La sesión guardada debe estar cifrada para usar esta aplicación.",
    ),
    (
        "The saved profile identity must be private.",
        "La identidad del perfil guardado debe ser privada.",
    ),
    (
        "The saved profile identity is invalid.",
        "La identidad del perfil guardado no es válida.",
    ),
    // Connection
    (
        "Connect to open your files in the file manager.",
        "Conéctate para abrir tus archivos en el gestor de archivos.",
    ),
    (
        "Enter the local key for your saved session.",
        "Ingresa la clave local de tu sesión guardada.",
    ),
    (
        "Install rclone, FUSE 3, and the desktop app before connecting.",
        "Instala rclone, FUSE 3 y la aplicación de escritorio antes de conectarte.",
    ),
    (
        "The iCloud folder must be empty before connecting.",
        "La carpeta de iCloud debe estar vacía antes de conectarte.",
    ),
    (
        "The saved session or application folders are unavailable.",
        "La sesión guardada o las carpetas de la aplicación no están disponibles.",
    ),
    ("Unlocking your saved session...", "Desbloqueando tu sesión guardada..."),
    (
        "No saved session was found. Select Sign in to set up your account.",
        "No se encontró una sesión guardada. Selecciona Iniciar sesión para configurar tu cuenta.",
    ),
    (
        "The saved session must be encrypted before connecting.",
        "La sesión guardada debe estar cifrada para conectarte.",
    ),
    (
        "Could not unlock the saved session. Check your local key.",
        "No se pudo desbloquear la sesión guardada. Revisa tu clave local.",
    ),
    (
        "The saved session has no iCloud Drive connection. Select Sign in to add it.",
        "La sesión guardada no tiene una conexión de iCloud Drive. Selecciona Iniciar sesión para agregarla.",
    ),
    (
        "The session check could not be started.",
        "No se pudo iniciar la comprobación de la sesión.",
    ),
    (
        "Another connection is using the iCloud folder. It was left unchanged.",
        "Otra conexión está usando la carpeta de iCloud. No se modificó.",
    ),
    ("Connecting to iCloud Drive...", "Conectando con iCloud Drive..."),
    (
        "iCloud Drive is ready to open in your file manager.",
        "iCloud Drive está listo para abrirse en tu gestor de archivos.",
    ),
    (
        "The connection stopped. Check your network or renew the saved session and reconnect.",
        "La conexión se detuvo. Revisa tu red o renueva la sesión guardada y vuelve a conectarte.",
    ),
    ("Closing the connection...", "Cerrando la conexión..."),
    (
        "The connection timed out. Check your network and try again.",
        "Se agotó el tiempo de conexión. Revisa tu red y vuelve a intentarlo.",
    ),
    (
        "Another filesystem is using the iCloud folder. It was left unchanged.",
        "Otro sistema de archivos está usando la carpeta de iCloud. No se modificó.",
    ),
    (
        "The connection changed outside this app. It was left unchanged.",
        "La conexión cambió fuera de esta aplicación. No se modificó.",
    ),
    (
        "An existing read-only connection is available. Reconnect here to enable uploads.",
        "Hay una conexión de solo lectura disponible. Vuelve a conectarte aquí para habilitar las subidas.",
    ),
    (
        "A previous connection is still mounted. Disconnect it before reconnecting.",
        "Una conexión anterior sigue montada. Desconéctala antes de volver a conectarte.",
    ),
    (
        "The mounted connection disappeared. Reconnect to try again.",
        "La conexión montada desapareció. Vuelve a conectarte para intentarlo de nuevo.",
    ),
    ("Disconnected from iCloud Drive.", "Desconectado de iCloud Drive."),
    (
        "The connection stopped. Reconnecting automatically in {seconds} seconds.",
        "La conexión se detuvo. Se reconectará automáticamente en {seconds} segundos.",
    ),
    (
        "The folder is used by another connection. It was left unchanged.",
        "Otra conexión usa la carpeta. No se modificó.",
    ),
    ("Disconnecting from iCloud Drive...", "Desconectando de iCloud Drive..."),
    (
        "The connection changed. It was left unchanged.",
        "La conexión cambió. No se modificó.",
    ),
    (
        "Could not disconnect. Close files in iCloud Drive and try again.",
        "No se pudo desconectar. Cierra los archivos de iCloud Drive y vuelve a intentarlo.",
    ),
    // Uploads
    ("Uploads", "Subidas"),
    ("Uploads ({count} pending)", "Subidas (en espera: {count})"),
    (
        "The saved upload queue could not be opened. Your pending files were preserved.",
        "No se pudo abrir la cola de subidas guardada. Tus archivos pendientes se conservaron.",
    ),
    ("&Upload to", "&Subir a"),
    (
        "Upload folder relative to iCloud Drive",
        "Carpeta de destino dentro de iCloud Drive",
    ),
    ("Add files...", "Agregar archivos..."),
    ("Choose files to upload", "Elige los archivos que quieres subir"),
    ("Upload queue", "Cola de subidas"),
    ("Resume uploads", "Reanudar subidas"),
    ("Pause uploads", "Pausar subidas"),
    ("Skip", "Omitir"),
    ("Keep both", "Conservar ambos"),
    ("Queued", "En cola"),
    ("Checking", "Comprobando"),
    ("Uploading", "Subiendo"),
    ("Verifying", "Verificando"),
    ("Completed", "Completada"),
    ("Conflict", "Conflicto"),
    ("Skipped", "Omitida"),
    (
        "Could not save the upload queue. Check local space and permissions, then resume uploads.",
        "No se pudo guardar la cola de subidas. Revisa el espacio local y los permisos, y después reanuda las subidas.",
    ),
    (
        "Connect and resume recovered pending uploads.",
        "Conéctate y reanuda las subidas pendientes recuperadas.",
    ),
    (
        "Choose files to upload. Existing cloud files are preserved.",
        "Elige los archivos que quieres subir. Los archivos que ya están en la nube se conservan.",
    ),
    (
        "Choose regular files outside the private application folders and a relative cloud folder.",
        "Elige archivos normales fuera de las carpetas privadas de la aplicación y una carpeta relativa en la nube.",
    ),
    (
        "Preparing private local copies...",
        "Preparando copias locales privadas...",
    ),
    (
        "Uploads resumed. Conflicts require a decision.",
        "Subidas reanudadas. Los conflictos requieren una decisión.",
    ),
    (
        "Files from a different saved session remain paused. Reconnect with their original profile to resume them.",
        "Los archivos de otra sesión guardada siguen en pausa. Vuelve a conectarte con su perfil original para reanudarlos.",
    ),
    (
        "Uploads paused. Pending local copies are retained.",
        "Subidas en pausa. Las copias locales pendientes se conservan.",
    ),
    (
        "Reconnect with the original saved session before keeping both versions.",
        "Vuelve a conectarte con la sesión guardada original antes de conservar ambas versiones.",
    ),
    (
        "The local copy is unavailable or changed. The cloud file was preserved.",
        "La copia local no está disponible o cambió. Se conservó el archivo de la nube.",
    ),
    (
        "Some files could not be prepared. Ready local copies are retained; check space and source files.",
        "Algunos archivos no se pudieron preparar. Las copias locales listas se conservan; revisa el espacio y los archivos de origen.",
    ),
    (
        "Files added to the upload queue.",
        "Archivos agregados a la cola de subidas.",
    ),
    (
        "Could not prepare the files. Check space, permissions, and whether they changed.",
        "No se pudieron preparar los archivos. Revisa el espacio, los permisos y si cambiaron.",
    ),
    (
        "A pending file belongs to another saved session. Its upload remains paused.",
        "Un archivo pendiente pertenece a otra sesión guardada. Su subida sigue en pausa.",
    ),
    (
        "Could not check the cloud file. Check your connection and resume.",
        "No se pudo comprobar el archivo de la nube. Revisa tu conexión y reanuda.",
    ),
    (
        "The cloud returned an unexpected file description. Resume to check again.",
        "La nube devolvió una descripción de archivo inesperada. Reanuda para comprobarlo de nuevo.",
    ),
    (
        "Could not verify the cloud copy. The local copy is retained; resume to check again.",
        "No se pudo verificar la copia en la nube. La copia local se conserva; reanuda para comprobarlo de nuevo.",
    ),
    (
        "Upload verified. The cloud copy matches the local snapshot.",
        "Subida verificada. La copia en la nube coincide con la copia local.",
    ),
    (
        "Could not open the destination folder. Check your connection and resume.",
        "No se pudo abrir la carpeta de destino. Revisa tu conexión y reanuda.",
    ),
    (
        "The local upload source changed. Its cloud destination was left untouched.",
        "El archivo local que se iba a subir cambió. Su destino en la nube no se modificó.",
    ),
    (
        "The upload stopped. The local copy is retained; resume to check the cloud and retry.",
        "La subida se detuvo. La copia local se conserva; reanuda para comprobar la nube y reintentar.",
    ),
    (
        "A different cloud file exists. Skip this upload or keep both under different names.",
        "Ya existe un archivo diferente en la nube. Omite esta subida o conserva ambos con nombres distintos.",
    ),
    (
        "Choose a regular filename without path separators or control characters.",
        "Elige un nombre de archivo normal, sin separadores de ruta ni caracteres de control.",
    ),
    (
        "Enter a folder relative to iCloud Drive.",
        "Ingresa una carpeta relativa a iCloud Drive.",
    ),
    (
        "Incomplete preparation files were preserved in the upload cache. Add their original files again.",
        "Los archivos de preparación incompletos se conservaron en la caché de subidas. Vuelve a agregar los archivos originales.",
    ),
    // Synced folders
    ("Folders on this computer", "Carpetas en este equipo"),
    ("Synced folders", "Carpetas sincronizadas"),
    ("Synced folder", "Carpeta sincronizada"),
    (
        "The synced folder list could not be opened. Synced folders were left unchanged.",
        "No se pudo abrir la lista de carpetas sincronizadas. Las carpetas sincronizadas no se modificaron.",
    ),
    (
        "Add an iCloud Drive folder to edit it in Files or Dolphin; changes sync both ways.",
        "Agrega una carpeta de iCloud Drive para editarla en Archivos o Dolphin; los cambios se sincronizan en ambos sentidos.",
    ),
    (
        "Choose an iCloud Drive folder to sync",
        "Elige una carpeta de iCloud Drive para sincronizar",
    ),
    ("Open", "Abrir"),
    ("Sync now", "Sincronizar ahora"),
    ("Backups", "Copias de seguridad"),
    ("Stop syncing", "Dejar de sincronizar"),
    ("Review...", "Revisar..."),
    ("Waiting", "En espera"),
    ("Paused", "En pausa"),
    ("Syncing", "Sincronizando"),
    ("Changes pending", "Cambios pendientes"),
    ("Up to date", "Al día"),
    ("Needs review", "Requiere revisión"),
    ("Error", "Error"),
    (
        "Apply the deletions or changes found in \"{name}\" to the other side? Files deleted from iCloud go to Recently Deleted; replaced or deleted local files are kept in Backups.",
        "¿Aplicar al otro lado las eliminaciones o cambios encontrados en \"{name}\"? Los archivos eliminados de iCloud van a Eliminados recientemente; los archivos locales reemplazados o eliminados se guardan en Copias de seguridad.",
    ),
    (
        "Copy files between \"{name}\" and iCloud to restore syncing? Nothing is deleted. Where both sides differ, the iCloud version is used and the local copy is kept in Backups.",
        "¿Copiar archivos entre \"{name}\" e iCloud para restablecer la sincronización? No se borra nada. Donde ambos lados difieran, se usará la versión de iCloud y la copia local se guardará en Copias de seguridad.",
    ),
    (
        "Stop syncing \"{name}\"? The local copy and the iCloud folder are kept.",
        "¿Dejar de sincronizar \"{name}\"? Se conservan la copia local y la carpeta de iCloud.",
    ),
    ("on this computer", "en este equipo"),
    ("in iCloud", "en iCloud"),
    ("on one side", "en uno de los lados"),
    (
        "Choose a folder inside iCloud Drive; syncing the whole drive is not supported.",
        "Elige una carpeta dentro de iCloud Drive; no se puede sincronizar todo iCloud Drive.",
    ),
    (
        "This folder is already synced or overlaps a synced folder.",
        "Esta carpeta ya está sincronizada o se superpone con una carpeta sincronizada.",
    ),
    (
        "The local synced folder could not be created.",
        "No se pudo crear la carpeta sincronizada local.",
    ),
    (
        "The synced folder list could not be saved. Check local space and permissions.",
        "No se pudo guardar la lista de carpetas sincronizadas. Revisa el espacio local y los permisos.",
    ),
    (
        "Connect to sync this folder.",
        "Conéctate para sincronizar esta carpeta.",
    ),
    (
        "Connect with the saved session that added this folder to sync it.",
        "Conéctate con la sesión guardada que agregó esta carpeta para sincronizarla.",
    ),
    (
        "{count} local change(s) waiting to sync.",
        "{count} cambio(s) local(es) en espera de sincronizarse.",
    ),
    (
        "The local synced folder must be a regular folder.",
        "La carpeta sincronizada local debe ser una carpeta normal.",
    ),
    (
        "The sync state folder is unavailable.",
        "La carpeta de estado de la sincronización no está disponible.",
    ),
    (
        "\"{first}\" and \"{second}\" differ only in letter case, which iCloud Drive treats as the same name. Rename one of them to continue syncing.",
        "\"{first}\" y \"{second}\" solo se distinguen por mayúsculas y minúsculas, e iCloud Drive los trata como el mismo nombre. Cambia el nombre de uno de ellos para seguir sincronizando.",
    ),
    (
        "Copying the iCloud folder to this computer...",
        "Copiando la carpeta de iCloud a este equipo...",
    ),
    ("Syncing...", "Sincronizando..."),
    (
        "Sync paused. It continues after you reconnect.",
        "Sincronización en pausa. Continuará cuando vuelvas a conectarte.",
    ),
    ("Up to date.", "Al día."),
    (
        "{count} conflict copy(ies) kept: the newer version keeps the name and the other ends with .conflict and a number.",
        "Se conservaron {count} copia(s) en conflicto: la versión más reciente conserva el nombre y la otra termina en .conflict y un número.",
    ),
    (
        "Replaced or deleted local files were saved in Backups.",
        "Los archivos locales reemplazados o eliminados se guardaron en Copias de seguridad.",
    ),
    (
        "Applying a few deletions, renames, or edits...",
        "Aplicando algunas eliminaciones, cambios de nombre o ediciones...",
    ),
    (
        "{count} of {total} items were deleted or renamed {side} since the last sync, so nothing was synced. Continue only if that was intended.",
        "Desde la última sincronización se eliminaron o renombraron {count} de {total} elementos {side}, así que no se sincronizó nada. Continúa solo si fue intencional.",
    ),
    (
        "Every item changed {side} since the last sync, for example after a clock change, so nothing was synced. Continue only if that was intended.",
        "Todos los elementos cambiaron {side} desde la última sincronización, por ejemplo después de un cambio de hora, así que no se sincronizó nada. Continúa solo si fue intencional.",
    ),
    (
        "The synced folder is empty or missing {side}, so nothing was synced; sync never removes everything at once. Continue to copy the files back, or stop syncing to keep both sides as they are.",
        "La carpeta sincronizada está vacía o no existe {side}, así que no se sincronizó nada; la sincronización nunca borra todo de una vez. Continúa para volver a copiar los archivos o deja de sincronizar para conservar ambos lados como están.",
    ),
    (
        "The sync history is missing or could not be trusted. Continue to restore it; where both sides differ, the iCloud version is used and the local copy is kept in Backups.",
        "Falta el historial de sincronización o no es confiable. Continúa para restaurarlo; donde ambos lados difieran, se usará la versión de iCloud y la copia local se guardará en Copias de seguridad.",
    ),
    (
        "Sync failed and will retry in a minute. Check the connection and free space.",
        "La sincronización falló y se reintentará en un minuto. Revisa la conexión y el espacio libre.",
    ),
    // Native dialogs
    ("Disconnect and renew", "Desconectar y renovar"),
    ("Turn on", "Activar"),
];
