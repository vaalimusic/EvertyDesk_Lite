use crate::launcher_store::LanguagePreference;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextKey {
    LanguageTitle,
    LanguageDescription,
    LanguageSaved,
    UpdatesTitle,
    UpdatesDescription,
    UpdatesDisabledHint,
    UpdatesManifestPlaceholder,
    UpdatesGithubPlaceholder,
    UpdatesChannelNotConfigured,
    UpdatesCurrentVersion,
    UpdatesCheck,
    UpdatesChecking,
    UpdatesUpToDate,
    UpdatesCheckAgain,
    UpdatesAvailable,
    UpdatesDownloadAndVerify,
    UpdatesDownloading,
    UpdatesReadyToInstall,
    UpdatesInstall,
    UpdatesRetry,
    UpdateChannelSaved,
    UpdateManifestUrlSaved,
    UpdateGithubRepoSaved,
    NavHome,
    NavAddressBook,
    NavSettings,
    SettingsSectionSecurity,
    SettingsSectionGeneral,
    SettingsSectionConnection,
    SettingsHintSecurity,
    SettingsHintGeneral,
    SettingsHintConnection,
    SettingsSectionsTitle,
    LanguageSystem,
    LanguageRussian,
    LanguageEnglish,
    LanguageSystemHint,
    LanguageRussianHint,
    LanguageEnglishHint,
    UpdateChannelDisabled,
    UpdateChannelManifestUrl,
    UpdateChannelGithubRelease,
    UpdateChannelDisabledHint,
    UpdateChannelManifestUrlHint,
    UpdateChannelGithubReleaseHint,
    HomeCredentialTitle,
    HomeCredentialSubtitlePrefix,
    HomeRemotePasswordPlaceholder,
    HomeRememberPassword,
    HomeRememberPasswordHint,
    HomeCancel,
    HomeConnect,
    HomeStopReceiving,
    HomeEnableAccess,
    HomeHide,
    HomeShow,
    HomeThisWorkspace,
    HomeCopy,
    HomeOneTimePassword,
    HomeRefreshNow,
    HomeRemoteAddressPlaceholder,
    HomeFavorites,
    HomeRecentSessions,
    HomeRecentEmptyTitle,
    HomeRecentEmptyHint,
    HomeRemoteDevice,
    SettingsTitle,
    SettingsSubtitle,
    SettingsPermanentPassword,
    SettingsPermanentPasswordDescription,
    SettingsPermanentPasswordPlaceholder,
    SettingsTemporaryPasswordRotates,
    SettingsDelete,
    SettingsSave,
    SettingsIncomingTitle,
    SettingsIncomingDescription,
    SettingsAlwaysAskConfirmation,
    SettingsAlwaysAskConfirmationHint,
    SettingsAccessAutoTitle,
    SettingsAccessAutoHint,
    SettingsPermissionsTitle,
    SettingsPermissionsDescription,
    SettingsKeyboardMouse,
    SettingsKeyboardMouseHint,
    SettingsSharedClipboard,
    SettingsSharedClipboardHint,
    SettingsOutgoingTitle,
    SettingsOutgoingDescription,
    SettingsImageQuality,
    SettingsQualityHint,
    QualitySmooth,
    QualityBalanced,
    QualitySharp,
    SettingsStreamingMode,
    StreamingModeSupportHint,
    StreamingModeInteractiveHint,
    StreamingModeGameHint,
    SettingsFsrUpscale,
    SettingsFsrHint,
    SettingsPlayRemoteAudio,
    SettingsPlayRemoteAudioHint,
    SettingsAppBehaviorTitle,
    SettingsAppBehaviorDescription,
    SettingsLaunchOnStartup,
    SettingsLaunchOnStartupHint,
    SettingsShowStartMenuShortcut,
    SettingsShowStartMenuShortcutHint,
    SettingsKeepTaskbarIcon,
    SettingsKeepTaskbarIconHintOn,
    SettingsKeepTaskbarIconHintOff,
    SettingsSmartAgentTitle,
    SettingsSmartAgentDescription,
    SettingsSmartAgentAvailable,
    SettingsSmartAgentEnable,
    SettingsSmartAgentServiceKeyPlaceholder,
    SettingsSmartAgentIdleHint,
    SettingsCompatibilityTitle,
    SettingsCompatibilityCustom,
    SettingsCompatibilityDefault,
    SettingsCompatibilityHide,
    SettingsCompatibilityShow,
    SettingsCompatibilityDiscover,
    SettingsCompatibilityDiscovering,
    SettingsCompatibilityDiscoveryHint,
    SettingsCompatibilityEmptyFieldsHint,
    SettingsNetworkDebugTitle,
    SettingsNetworkDebugDescription,
    SettingsNetworkDebugIgnoreLan,
    SettingsNetworkDebugIgnoreLanHint,
    SettingsNetworkDebugForceRelay,
    SettingsNetworkDebugForceRelayHint,
    SettingsReset,
    AboutTitle,
    AboutSubtitle,
    AboutAuthor,
    AboutVersion,
    AboutGithub,
    AboutHabr,
    AboutContact,
    AboutDesk,
    AboutDeskDescription,
    AboutCheckUpdates,
    AboutClose,
    AboutCopyEmail,
    AddressBookTitle,
    AddressBookSubtitle,
    AddressBookNoGroup,
    AddressBookDeviceId,
    AddressBookLocalCloudDevices,
    AddressBookHideContactForm,
    AddressBookAddNewContact,
    AddressBookNoSavedDevices,
    AddressBookNoSavedDevicesHint,
    AddressBookContactsNotFound,
    AddressBookTryChangeSearch,
    AddressBookRemoveFromFavorites,
    AddressBookAddToFavorites,
    AddressBookShowDetails,
    AddressBookEditContact,
    AddressBookConnect,
    AddressBookDeleteContact,
    AddressBookRecentTitle,
    AddressBookRecentDescription,
    AddressBookClearHistory,
    AddressBookHistoryEmpty,
    AddressBookHistoryEmptyHint,
    AddressBookHistoryNotFound,
    AddressBookSelectAddress,
    AddressBookRemoveFromHistory,
    AddressBookEditing,
    AddressBookNewContact,
    AddressBookNameAndIdRequired,
    AddressBookCloseForm,
    AddressBookDeviceNamePlaceholder,
    AddressBookGroupPathPlaceholder,
    AddressBookTagsPlaceholder,
    AddressBookNotePlaceholder,
    AddressBookGroups,
    AddressBookTags,
    AddressBookSave,
    AddressBookAdd,
    AddressBookClear,
    AddressBookAllContacts,
    AddressBookFavorites,
    AddressBookRecent,
    AddressBookRecentContacts,
    AddressBookGroupContacts,
    AddressBookTaggedContacts,
    AddressBookAllShort,
    AddressBookResetFilter,
    AddressBookSearchPlaceholder,
    AddressBookShownTotalTemplate,
    AddressBookSync,
    AddressBookSyncRestored,
    AddressBookSyncAvailable,
    AddressBookSyncEnabled,
    AddressBookRefreshEntitlements,
    AddressBookSignOutCloud,
    AddressBookSignIn,
    AddressBookSigningIn,
    AddressBookYandex,
    AddressBookWaitingYandex,
    AddressBookCancel,
    AddressBookLocalWorks,
    AddressBookLocalTitle,
    AddressBookLoginPlaceholder,
    AddressBookPasswordPlaceholder,
    AddressBookContactDetails,
    AddressBookQuickActions,
    AddressBookHideDetails,
    AddressBookCopyId,
    AddressBookUseAddress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiLanguage {
    Russian,
    English,
}

impl UiLanguage {
    pub fn from_preference(preference: LanguagePreference) -> Self {
        match preference {
            LanguagePreference::System => system_language(),
            LanguagePreference::Russian => Self::Russian,
            LanguagePreference::English => Self::English,
        }
    }
}

pub fn tr(language: UiLanguage, key: TextKey) -> &'static str {
    match language {
        UiLanguage::Russian => ru(key),
        UiLanguage::English => en(key),
    }
}

fn system_language() -> UiLanguage {
    #[cfg(windows)]
    if let Some(language) = windows_user_language() {
        return language;
    }

    let locale = std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    language_from_locale(&locale).unwrap_or(UiLanguage::English)
}

fn language_from_locale(locale: &str) -> Option<UiLanguage> {
    let normalized = locale.trim().to_ascii_lowercase();
    if normalized.starts_with("ru") {
        Some(UiLanguage::Russian)
    } else if normalized.is_empty() {
        None
    } else {
        Some(UiLanguage::English)
    }
}

#[cfg(windows)]
fn windows_user_language() -> Option<UiLanguage> {
    let mut locale = [0u16; 85];
    let len = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut locale) };
    if len <= 1 {
        return None;
    }
    let locale = String::from_utf16_lossy(&locale[..(len as usize - 1)]);
    language_from_locale(&locale)
}

fn ru(key: TextKey) -> &'static str {
    match key {
        TextKey::LanguageTitle => "Язык интерфейса",
        TextKey::LanguageDescription => {
            "Каркас мультиязычности: настройка уже сохраняется, строки UI постепенно переносятся в словари переводов."
        }
        TextKey::LanguageSaved => "Язык интерфейса сохранён",
        TextKey::UpdatesTitle => "Обновления",
        TextKey::UpdatesDescription => {
            "Можно проверять HTTPS manifest напрямую или брать latest.json из последнего GitHub Release."
        }
        TextKey::UpdatesDisabledHint => {
            "Проверка отключена. Для CI/portable остаётся fallback через EVERTYDESK_UPDATE_URL."
        }
        TextKey::UpdatesManifestPlaceholder => "https://example.com/latest.json",
        TextKey::UpdatesGithubPlaceholder => "owner/repo, например vaalimusic/EvertyDesk_Lite",
        TextKey::UpdatesChannelNotConfigured => "Канал обновлений не настроен",
        TextKey::UpdatesCurrentVersion => "Текущая версия",
        TextKey::UpdatesCheck => "Проверить обновления",
        TextKey::UpdatesChecking => "Проверка обновлений...",
        TextKey::UpdatesUpToDate => "Установлена последняя версия",
        TextKey::UpdatesCheckAgain => "Проверить снова",
        TextKey::UpdatesAvailable => "Доступно обновление",
        TextKey::UpdatesDownloadAndVerify => "Скачать и проверить",
        TextKey::UpdatesDownloading => "Загрузка обновления",
        TextKey::UpdatesReadyToInstall => {
            "Обновление загружено и проверено — готово к установке."
        }
        TextKey::UpdatesInstall => "Установить",
        TextKey::UpdatesRetry => "Повторить",
        TextKey::UpdateChannelSaved => "Канал обновлений сохранён",
        TextKey::UpdateManifestUrlSaved => "Manifest URL обновлений сохранён",
        TextKey::UpdateGithubRepoSaved => "GitHub repository для обновлений сохранён",
        TextKey::NavHome => "Главная",
        TextKey::NavAddressBook => "Адресная книга",
        TextKey::NavSettings => "Настройки",
        TextKey::SettingsSectionSecurity => "Безопасность",
        TextKey::SettingsSectionGeneral => "Общее",
        TextKey::SettingsSectionConnection => "Подключение",
        TextKey::SettingsHintSecurity => "Пароли, подтверждение и права входящих сессий",
        TextKey::SettingsHintGeneral => "Качество, режимы, звук и интеграции",
        TextKey::SettingsHintConnection => "RustDesk-совместимые серверы и состояние",
        TextKey::SettingsSectionsTitle => "Разделы",
        TextKey::LanguageSystem => "Система",
        TextKey::LanguageRussian => "Русский",
        TextKey::LanguageEnglish => "English",
        TextKey::LanguageSystemHint => {
            "Использовать язык операционной системы, если перевод есть."
        }
        TextKey::LanguageRussianHint => "Русский интерфейс.",
        TextKey::LanguageEnglishHint => "English interface.",
        TextKey::UpdateChannelDisabled => "Отключено",
        TextKey::UpdateChannelManifestUrl => "Manifest URL",
        TextKey::UpdateChannelGithubRelease => "GitHub Releases",
        TextKey::UpdateChannelDisabledHint => {
            "Автоматическая проверка обновлений не выполняется."
        }
        TextKey::UpdateChannelManifestUrlHint => {
            "Проверять HTTPS latest.json с вашим manifest-контрактом."
        }
        TextKey::UpdateChannelGithubReleaseHint => {
            "Искать latest.json в последнем GitHub Release."
        }
        TextKey::HomeCredentialTitle => "Авторизация",
        TextKey::HomeCredentialSubtitlePrefix => "Подключение к",
        TextKey::HomeRemotePasswordPlaceholder => "Пароль удалённого устройства",
        TextKey::HomeRememberPassword => "Запомнить пароль на этом компьютере",
        TextKey::HomeRememberPasswordHint => {
            "Пароль хранится в Windows Credential Manager и не записывается в настройки EvertyDesk."
        }
        TextKey::HomeCancel => "Отмена",
        TextKey::HomeConnect => "Подключиться",
        TextKey::HomeStopReceiving => "Остановить приём",
        TextKey::HomeEnableAccess => "Включить доступ",
        TextKey::HomeHide => "Скрыть",
        TextKey::HomeShow => "Показать",
        TextKey::HomeThisWorkspace => "Это рабочее место",
        TextKey::HomeCopy => "Копировать",
        TextKey::HomeOneTimePassword => "Одноразовый пароль",
        TextKey::HomeRefreshNow => "Обновить сейчас",
        TextKey::HomeRemoteAddressPlaceholder => "Введите удалённый адрес",
        TextKey::HomeFavorites => "Избранное",
        TextKey::HomeRecentSessions => "Недавние сеансы",
        TextKey::HomeRecentEmptyTitle => "Здесь появятся последние подключения",
        TextKey::HomeRecentEmptyHint => {
            "Введите удалённый адрес в верхней строке, чтобы начать."
        }
        TextKey::HomeRemoteDevice => "Удалённое устройство",
        TextKey::SettingsTitle => "Настройки",
        TextKey::SettingsSubtitle => "Безопасность входящих подключений и поведение приложения",
        TextKey::SettingsPermanentPassword => "Постоянный пароль",
        TextKey::SettingsPermanentPasswordDescription => {
            "Используется для unattended-доступа. Хранится отдельно в системном credential store, не в config.json."
        }
        TextKey::SettingsPermanentPasswordPlaceholder => "Задайте постоянный пароль",
        TextKey::SettingsTemporaryPasswordRotates => {
            "Одноразовый пароль обновляется автоматически каждые 10 минут."
        }
        TextKey::SettingsDelete => "Удалить",
        TextKey::SettingsSave => "Сохранить",
        TextKey::SettingsIncomingTitle => "Входящие подключения",
        TextKey::SettingsIncomingDescription => {
            "Определяет, как другие устройства получают доступ к этому компьютеру."
        }
        TextKey::SettingsAlwaysAskConfirmation => "Всегда запрашивать подтверждение",
        TextKey::SettingsAlwaysAskConfirmationHint => {
            "Показывать запрос «Принять / Отклонить» перед началом сессии."
        }
        TextKey::SettingsAccessAutoTitle => {
            "Доступ включается автоматически при открытии EvertyDesk"
        }
        TextKey::SettingsAccessAutoHint => {
            "Если нужно временно закрыть устройство для входящих подключений, используйте кнопку «Остановить приём» на главной странице."
        }
        TextKey::SettingsPermissionsTitle => "Разрешения сессии",
        TextKey::SettingsPermissionsDescription => {
            "Ограничения применяются ко всем новым входящим подключениям."
        }
        TextKey::SettingsKeyboardMouse => "Клавиатура и мышь",
        TextKey::SettingsKeyboardMouseHint => {
            "Разрешить удалённому пользователю управлять системой."
        }
        TextKey::SettingsSharedClipboard => "Общий буфер обмена",
        TextKey::SettingsSharedClipboardHint => {
            "Синхронизировать текст при копировании и вставке."
        }
        TextKey::SettingsOutgoingTitle => "Исходящие подключения",
        TextKey::SettingsOutgoingDescription => {
            "Значения по умолчанию для новых удалённых сессий."
        }
        TextKey::SettingsImageQuality => "Качество изображения",
        TextKey::SettingsQualityHint => "Профиль можно изменить во время активной сессии.",
        TextKey::QualitySmooth => "Плавность",
        TextKey::QualityBalanced => "Баланс",
        TextKey::QualitySharp => "Качество",
        TextKey::SettingsStreamingMode => "Режим трансляции",
        TextKey::StreamingModeSupportHint => {
            "Обычная удалённая поддержка: стабильность и экономия трафика."
        }
        TextKey::StreamingModeInteractiveHint => {
            "Баланс реакции и качества для повседневной работы."
        }
        TextKey::StreamingModeGameHint => {
            "Минимальная задержка: 60 FPS, adaptive quality выключается."
        }
        TextKey::SettingsFsrUpscale => "Апскейл изображения (FSR)",
        TextKey::SettingsFsrHint => {
            "Дорезчает картинку, если хост передаёт кадр ниже своего нативного разрешения — полезно с «Поддержкой»/«Игрой» на медленной сети. По умолчанию выключен."
        }
        TextKey::SettingsPlayRemoteAudio => "Воспроизводить звук удалённого компьютера",
        TextKey::SettingsPlayRemoteAudioHint => {
            "Звук также можно отключить отдельно в окне viewer."
        }
        TextKey::SettingsAppBehaviorTitle => "Поведение приложения",
        TextKey::SettingsAppBehaviorDescription => {
            "Автозапуск и поведение главного окна при нажатии крестика."
        }
        TextKey::SettingsLaunchOnStartup => "Запускать EvertyDesk при входе в Windows",
        TextKey::SettingsLaunchOnStartupHint => {
            "Добавляет текущий exe в автозагрузку пользователя. Права администратора не нужны."
        }
        TextKey::SettingsShowStartMenuShortcut => "Показывать EvertyDesk в меню Пуск",
        TextKey::SettingsShowStartMenuShortcutHint => {
            "Создаёт пользовательский ярлык для portable-версии. Установщик MSI также создаёт свой ярлык автоматически."
        }
        TextKey::SettingsKeepTaskbarIcon => {
            "Оставлять кнопку на панели задач при закрытии"
        }
        TextKey::SettingsKeepTaskbarIconHintOn => {
            "Крестик будет сворачивать окно. EvertyDesk останется и на панели задач, и в трее."
        }
        TextKey::SettingsKeepTaskbarIconHintOff => {
            "Крестик будет скрывать окно полностью. EvertyDesk останется только в системном трее."
        }
        TextKey::SettingsSmartAgentTitle => "Интеграция с desk.everty.ru",
        TextKey::SettingsSmartAgentDescription => {
            "Регистрация устройства и сообщения Smart Agent."
        }
        TextKey::SettingsSmartAgentAvailable => {
            "Smart Agent доступен в правах аккаунта."
        }
        TextKey::SettingsSmartAgentEnable => "Включить Smart Agent",
        TextKey::SettingsSmartAgentServiceKeyPlaceholder => {
            "Ключ организации (service_key)"
        }
        TextKey::SettingsSmartAgentIdleHint => {
            "Heartbeat отправляется раз в минуту, новые сообщения проверяются раз в 30 секунд."
        }
        TextKey::SettingsCompatibilityTitle => "RustDesk-совместимость и серверы",
        TextKey::SettingsCompatibilityCustom => "Используется другой ID/Relay/API сервер",
        TextKey::SettingsCompatibilityDefault => {
            "Используются встроенные серверы EvertyDesk"
        }
        TextKey::SettingsCompatibilityHide => "Скрыть параметры",
        TextKey::SettingsCompatibilityShow => "Показать параметры",
        TextKey::SettingsCompatibilityDiscover => "Получить из API",
        TextKey::SettingsCompatibilityDiscovering => "Проверка…",
        TextKey::SettingsCompatibilityDiscoveryHint => {
            "GET /public/connection заполняет ID/Relay и Public Key, если токен имеет право."
        }
        TextKey::SettingsCompatibilityEmptyFieldsHint => {
            "Пустые поля означают встроенные серверы EvertyDesk. Ваши значения сохраняются только локально."
        }
        TextKey::SettingsNetworkDebugTitle => "Отладка сети",
        TextKey::SettingsNetworkDebugDescription => {
            "Помогает проверить маршрутизацию EVRTCK вне обычного LAN-сценария."
        }
        TextKey::SettingsNetworkDebugIgnoreLan => "Игнорировать LAN-кандидаты",
        TextKey::SettingsNetworkDebugIgnoreLanHint => {
            "Viewer не будет использовать локальные адреса 10.x, 172.16-31.x, 192.168.x, loopback и link-local."
        }
        TextKey::SettingsNetworkDebugForceRelay => "Принудительно использовать relay",
        TextKey::SettingsNetworkDebugForceRelayHint => {
            "Отключает прямые UDP/TCP-пробы. Используйте для проверки поведения через сервер-посредник."
        }
        TextKey::SettingsReset => "Сбросить",
        TextKey::AboutTitle => "О EvertyDesk",
        TextKey::AboutSubtitle => "EvertyDesk Next 2 — удалённый рабочий стол и адресная книга",
        TextKey::AboutAuthor => "Автор",
        TextKey::AboutVersion => "Версия",
        TextKey::AboutGithub => "GitHub",
        TextKey::AboutHabr => "Хабр",
        TextKey::AboutContact => "Связь",
        TextKey::AboutDesk => "desk.everty.ru",
        TextKey::AboutDeskDescription => {
            "Облачная адресная книга, авторизация, операторы и Smart Agent."
        }
        TextKey::AboutCheckUpdates => "Проверить обновления",
        TextKey::AboutClose => "Закрыть",
        TextKey::AboutCopyEmail => "Скопировать email",
        TextKey::AddressBookTitle => "Адресная книга",
        TextKey::AddressBookSubtitle => "Контакты, группы, заметки и история подключений",
        TextKey::AddressBookNoGroup => "Без группы",
        TextKey::AddressBookDeviceId => "ID устройства",
        TextKey::AddressBookLocalCloudDevices => "Локальные и облачные устройства",
        TextKey::AddressBookHideContactForm => "Скрыть форму контакта",
        TextKey::AddressBookAddNewContact => "Добавить новый контакт",
        TextKey::AddressBookNoSavedDevices => "Нет сохранённых устройств",
        TextKey::AddressBookNoSavedDevicesHint => {
            "Выберите адрес выше, задайте название и сохраните его."
        }
        TextKey::AddressBookContactsNotFound => "Контакты не найдены",
        TextKey::AddressBookTryChangeSearch => "Попробуйте изменить поисковый запрос.",
        TextKey::AddressBookRemoveFromFavorites => "Убрать из избранного",
        TextKey::AddressBookAddToFavorites => "Добавить в избранное",
        TextKey::AddressBookShowDetails => "Показать детали",
        TextKey::AddressBookEditContact => "Редактировать контакт",
        TextKey::AddressBookConnect => "Подключиться",
        TextKey::AddressBookDeleteContact => "Удалить контакт",
        TextKey::AddressBookRecentTitle => "Недавние",
        TextKey::AddressBookRecentDescription => "Последние адреса подключений",
        TextKey::AddressBookClearHistory => "Очистить историю",
        TextKey::AddressBookHistoryEmpty => "История пока пуста",
        TextKey::AddressBookHistoryEmptyHint => "Здесь появятся последние подключения.",
        TextKey::AddressBookHistoryNotFound => "В истории ничего не найдено",
        TextKey::AddressBookSelectAddress => "Подставить адрес",
        TextKey::AddressBookRemoveFromHistory => "Удалить из истории",
        TextKey::AddressBookEditing => "Редактирование",
        TextKey::AddressBookNewContact => "Новый контакт",
        TextKey::AddressBookNameAndIdRequired => "Имя и ID обязательны",
        TextKey::AddressBookCloseForm => "Закрыть форму",
        TextKey::AddressBookDeviceNamePlaceholder => "Название устройства",
        TextKey::AddressBookGroupPathPlaceholder => "Группа / путь",
        TextKey::AddressBookTagsPlaceholder => "Метки через запятую",
        TextKey::AddressBookNotePlaceholder => "Заметка",
        TextKey::AddressBookGroups => "Группы",
        TextKey::AddressBookTags => "Метки",
        TextKey::AddressBookSave => "Сохранить",
        TextKey::AddressBookAdd => "Добавить",
        TextKey::AddressBookClear => "Очистить",
        TextKey::AddressBookAllContacts => "Все контакты",
        TextKey::AddressBookFavorites => "Избранные",
        TextKey::AddressBookRecent => "Недавние",
        TextKey::AddressBookRecentContacts => "Недавние контакты",
        TextKey::AddressBookGroupContacts => "Контакты группы",
        TextKey::AddressBookTaggedContacts => "Контакты с меткой",
        TextKey::AddressBookAllShort => "Все",
        TextKey::AddressBookResetFilter => "Сбросить фильтр адресной книги",
        TextKey::AddressBookSearchPlaceholder => {
            "Поиск по имени, ID, группе, метке или заметке"
        }
        TextKey::AddressBookShownTotalTemplate => "{} показано · {} всего",
        TextKey::AddressBookSync => "Синхронизировать адресную книгу",
        TextKey::AddressBookSyncRestored => {
            "Вход восстановлен. Запустите синхронизацию, чтобы обновить облачные контакты."
        }
        TextKey::AddressBookSyncAvailable => {
            "Локальные контакты доступны всегда; облачные контакты синхронизируются вручную."
        }
        TextKey::AddressBookSyncEnabled => "Синхронизация включена",
        TextKey::AddressBookRefreshEntitlements => "Обновить права аккаунта",
        TextKey::AddressBookSignOutCloud => "Выйти из облачной адресной книги",
        TextKey::AddressBookSignIn => "Войти",
        TextKey::AddressBookSigningIn => "Вход…",
        TextKey::AddressBookYandex => "Яндекс",
        TextKey::AddressBookWaitingYandex => "Ожидаю Яндекс…",
        TextKey::AddressBookCancel => "Отмена",
        TextKey::AddressBookLocalWorks => {
            "Локальная адресная книга работает без входа. Авторизация нужна только для облачной синхронизации."
        }
        TextKey::AddressBookLocalTitle => "Локальная адресная книга",
        TextKey::AddressBookLoginPlaceholder => "Логин или e-mail",
        TextKey::AddressBookPasswordPlaceholder => "Пароль или токен",
        TextKey::AddressBookContactDetails => "Детали контакта",
        TextKey::AddressBookQuickActions => "Быстрые действия без открытия формы",
        TextKey::AddressBookHideDetails => "Скрыть детали",
        TextKey::AddressBookCopyId => "Скопировать ID",
        TextKey::AddressBookUseAddress => "Подставить адрес",
    }
}

fn en(key: TextKey) -> &'static str {
    match key {
        TextKey::LanguageTitle => "Interface language",
        TextKey::LanguageDescription => {
            "Multilanguage foundation: the setting is persisted, and UI strings are being moved into translation dictionaries."
        }
        TextKey::LanguageSaved => "Interface language saved",
        TextKey::UpdatesTitle => "Updates",
        TextKey::UpdatesDescription => {
            "Check an HTTPS manifest directly or read latest.json from the latest GitHub Release."
        }
        TextKey::UpdatesDisabledHint => {
            "Checks are disabled. CI/portable builds can still use the EVERTYDESK_UPDATE_URL fallback."
        }
        TextKey::UpdatesManifestPlaceholder => "https://example.com/latest.json",
        TextKey::UpdatesGithubPlaceholder => "owner/repo, for example vaalimusic/EvertyDesk_Lite",
        TextKey::UpdatesChannelNotConfigured => "Update channel is not configured",
        TextKey::UpdatesCurrentVersion => "Current version",
        TextKey::UpdatesCheck => "Check for updates",
        TextKey::UpdatesChecking => "Checking for updates...",
        TextKey::UpdatesUpToDate => "Latest version is installed",
        TextKey::UpdatesCheckAgain => "Check again",
        TextKey::UpdatesAvailable => "Update available",
        TextKey::UpdatesDownloadAndVerify => "Download and verify",
        TextKey::UpdatesDownloading => "Downloading update",
        TextKey::UpdatesReadyToInstall => "Update downloaded and verified — ready to install.",
        TextKey::UpdatesInstall => "Install",
        TextKey::UpdatesRetry => "Retry",
        TextKey::UpdateChannelSaved => "Update channel saved",
        TextKey::UpdateManifestUrlSaved => "Update manifest URL saved",
        TextKey::UpdateGithubRepoSaved => "GitHub repository for updates saved",
        TextKey::NavHome => "Home",
        TextKey::NavAddressBook => "Address book",
        TextKey::NavSettings => "Settings",
        TextKey::SettingsSectionSecurity => "Security",
        TextKey::SettingsSectionGeneral => "General",
        TextKey::SettingsSectionConnection => "Connection",
        TextKey::SettingsHintSecurity => "Passwords, approval, and incoming session permissions",
        TextKey::SettingsHintGeneral => "Quality, modes, audio, and integrations",
        TextKey::SettingsHintConnection => "RustDesk-compatible servers and status",
        TextKey::SettingsSectionsTitle => "Sections",
        TextKey::LanguageSystem => "System",
        TextKey::LanguageRussian => "Русский",
        TextKey::LanguageEnglish => "English",
        TextKey::LanguageSystemHint => "Use the operating system language when a translation exists.",
        TextKey::LanguageRussianHint => "Russian interface.",
        TextKey::LanguageEnglishHint => "English interface.",
        TextKey::UpdateChannelDisabled => "Disabled",
        TextKey::UpdateChannelManifestUrl => "Manifest URL",
        TextKey::UpdateChannelGithubRelease => "GitHub Releases",
        TextKey::UpdateChannelDisabledHint => "Automatic update checks are disabled.",
        TextKey::UpdateChannelManifestUrlHint => "Check your HTTPS latest.json manifest contract.",
        TextKey::UpdateChannelGithubReleaseHint => "Find latest.json in the latest GitHub Release.",
        TextKey::HomeCredentialTitle => "Authorization",
        TextKey::HomeCredentialSubtitlePrefix => "Connecting to",
        TextKey::HomeRemotePasswordPlaceholder => "Remote device password",
        TextKey::HomeRememberPassword => "Remember password on this computer",
        TextKey::HomeRememberPasswordHint => {
            "The password is stored in Windows Credential Manager and is not written to EvertyDesk settings."
        }
        TextKey::HomeCancel => "Cancel",
        TextKey::HomeConnect => "Connect",
        TextKey::HomeStopReceiving => "Stop receiving",
        TextKey::HomeEnableAccess => "Enable access",
        TextKey::HomeHide => "Hide",
        TextKey::HomeShow => "Show",
        TextKey::HomeThisWorkspace => "This workspace",
        TextKey::HomeCopy => "Copy",
        TextKey::HomeOneTimePassword => "One-time password",
        TextKey::HomeRefreshNow => "Refresh now",
        TextKey::HomeRemoteAddressPlaceholder => "Enter remote address",
        TextKey::HomeFavorites => "Favorites",
        TextKey::HomeRecentSessions => "Recent sessions",
        TextKey::HomeRecentEmptyTitle => "Recent connections will appear here",
        TextKey::HomeRecentEmptyHint => "Enter a remote address in the top bar to start.",
        TextKey::HomeRemoteDevice => "Remote device",
        TextKey::SettingsTitle => "Settings",
        TextKey::SettingsSubtitle => "Incoming connection security and application behavior",
        TextKey::SettingsPermanentPassword => "Permanent password",
        TextKey::SettingsPermanentPasswordDescription => {
            "Used for unattended access. Stored separately in the system credential store, not in config.json."
        }
        TextKey::SettingsPermanentPasswordPlaceholder => "Set a permanent password",
        TextKey::SettingsTemporaryPasswordRotates => {
            "The one-time password is refreshed automatically every 10 minutes."
        }
        TextKey::SettingsDelete => "Delete",
        TextKey::SettingsSave => "Save",
        TextKey::SettingsIncomingTitle => "Incoming connections",
        TextKey::SettingsIncomingDescription => {
            "Controls how other devices get access to this computer."
        }
        TextKey::SettingsAlwaysAskConfirmation => "Always ask for confirmation",
        TextKey::SettingsAlwaysAskConfirmationHint => {
            "Show an Accept / Decline prompt before starting a session."
        }
        TextKey::SettingsAccessAutoTitle => "Access is enabled automatically when EvertyDesk opens",
        TextKey::SettingsAccessAutoHint => {
            "To temporarily close this device for incoming connections, use Stop receiving on the Home page."
        }
        TextKey::SettingsPermissionsTitle => "Session permissions",
        TextKey::SettingsPermissionsDescription => {
            "Restrictions apply to all new incoming connections."
        }
        TextKey::SettingsKeyboardMouse => "Keyboard and mouse",
        TextKey::SettingsKeyboardMouseHint => "Allow the remote user to control the system.",
        TextKey::SettingsSharedClipboard => "Shared clipboard",
        TextKey::SettingsSharedClipboardHint => "Sync text while copying and pasting.",
        TextKey::SettingsOutgoingTitle => "Outgoing connections",
        TextKey::SettingsOutgoingDescription => "Defaults for new remote sessions.",
        TextKey::SettingsImageQuality => "Image quality",
        TextKey::SettingsQualityHint => "The profile can be changed during an active session.",
        TextKey::QualitySmooth => "Smooth",
        TextKey::QualityBalanced => "Balanced",
        TextKey::QualitySharp => "Quality",
        TextKey::SettingsStreamingMode => "Streaming mode",
        TextKey::StreamingModeSupportHint => {
            "Regular remote support: stable behavior and lower traffic."
        }
        TextKey::StreamingModeInteractiveHint => {
            "Balanced responsiveness and quality for everyday work."
        }
        TextKey::StreamingModeGameHint => {
            "Lowest latency: 60 FPS, adaptive quality is disabled."
        }
        TextKey::SettingsFsrUpscale => "Image upscaling (FSR)",
        TextKey::SettingsFsrHint => {
            "Sharpens the image when the host sends a frame below native resolution — useful with Support/Game on slow networks. Disabled by default."
        }
        TextKey::SettingsPlayRemoteAudio => "Play remote computer audio",
        TextKey::SettingsPlayRemoteAudioHint => {
            "Audio can also be disabled separately in the viewer window."
        }
        TextKey::SettingsAppBehaviorTitle => "Application behavior",
        TextKey::SettingsAppBehaviorDescription => {
            "Startup and main-window behavior when pressing the close button."
        }
        TextKey::SettingsLaunchOnStartup => "Start EvertyDesk when signing in to Windows",
        TextKey::SettingsLaunchOnStartupHint => {
            "Adds the current exe to the user's startup entries. Administrator rights are not required."
        }
        TextKey::SettingsShowStartMenuShortcut => "Show EvertyDesk in the Start menu",
        TextKey::SettingsShowStartMenuShortcutHint => {
            "Creates a user shortcut for the portable version. The MSI installer also creates its own shortcut automatically."
        }
        TextKey::SettingsKeepTaskbarIcon => "Keep the taskbar button when closing",
        TextKey::SettingsKeepTaskbarIconHintOn => {
            "The close button will minimize the window. EvertyDesk remains both on the taskbar and in the tray."
        }
        TextKey::SettingsKeepTaskbarIconHintOff => {
            "The close button will hide the window completely. EvertyDesk remains only in the system tray."
        }
        TextKey::SettingsSmartAgentTitle => "desk.everty.ru integration",
        TextKey::SettingsSmartAgentDescription => "Device registration and Smart Agent messages.",
        TextKey::SettingsSmartAgentAvailable => "Smart Agent is available in account entitlements.",
        TextKey::SettingsSmartAgentEnable => "Enable Smart Agent",
        TextKey::SettingsSmartAgentServiceKeyPlaceholder => "Organization key (service_key)",
        TextKey::SettingsSmartAgentIdleHint => {
            "Heartbeat is sent once a minute; new messages are checked every 30 seconds."
        }
        TextKey::SettingsCompatibilityTitle => "RustDesk compatibility and servers",
        TextKey::SettingsCompatibilityCustom => "A custom ID/Relay/API server is in use",
        TextKey::SettingsCompatibilityDefault => "Built-in EvertyDesk servers are in use",
        TextKey::SettingsCompatibilityHide => "Hide settings",
        TextKey::SettingsCompatibilityShow => "Show settings",
        TextKey::SettingsCompatibilityDiscover => "Get from API",
        TextKey::SettingsCompatibilityDiscovering => "Checking…",
        TextKey::SettingsCompatibilityDiscoveryHint => {
            "GET /public/connection fills ID/Relay and Public Key when the token has permission."
        }
        TextKey::SettingsCompatibilityEmptyFieldsHint => {
            "Empty fields mean built-in EvertyDesk servers. Your values are stored only locally."
        }
        TextKey::SettingsNetworkDebugTitle => "Network debug",
        TextKey::SettingsNetworkDebugDescription => {
            "Helps test EVRTCK routing outside the normal LAN path."
        }
        TextKey::SettingsNetworkDebugIgnoreLan => "Ignore LAN candidates",
        TextKey::SettingsNetworkDebugIgnoreLanHint => {
            "The viewer will not use local 10.x, 172.16-31.x, 192.168.x, loopback, or link-local addresses."
        }
        TextKey::SettingsNetworkDebugForceRelay => "Force relay transport",
        TextKey::SettingsNetworkDebugForceRelayHint => {
            "Disables direct UDP/TCP probes. Use it to test behavior through the relay server."
        }
        TextKey::SettingsReset => "Reset",
        TextKey::AboutTitle => "About EvertyDesk",
        TextKey::AboutSubtitle => "EvertyDesk Next 2 — remote desktop and address book",
        TextKey::AboutAuthor => "Author",
        TextKey::AboutVersion => "Version",
        TextKey::AboutGithub => "GitHub",
        TextKey::AboutHabr => "Habr",
        TextKey::AboutContact => "Contact",
        TextKey::AboutDesk => "desk.everty.ru",
        TextKey::AboutDeskDescription => "Cloud address book, sign-in, operators, and Smart Agent.",
        TextKey::AboutCheckUpdates => "Check for updates",
        TextKey::AboutClose => "Close",
        TextKey::AboutCopyEmail => "Copy email",
        TextKey::AddressBookTitle => "Address book",
        TextKey::AddressBookSubtitle => "Contacts, groups, notes, and connection history",
        TextKey::AddressBookNoGroup => "No group",
        TextKey::AddressBookDeviceId => "Device ID",
        TextKey::AddressBookLocalCloudDevices => "Local and cloud devices",
        TextKey::AddressBookHideContactForm => "Hide contact form",
        TextKey::AddressBookAddNewContact => "Add new contact",
        TextKey::AddressBookNoSavedDevices => "No saved devices",
        TextKey::AddressBookNoSavedDevicesHint => "Choose an address above, name it, and save it.",
        TextKey::AddressBookContactsNotFound => "No contacts found",
        TextKey::AddressBookTryChangeSearch => "Try changing the search query.",
        TextKey::AddressBookRemoveFromFavorites => "Remove from favorites",
        TextKey::AddressBookAddToFavorites => "Add to favorites",
        TextKey::AddressBookShowDetails => "Show details",
        TextKey::AddressBookEditContact => "Edit contact",
        TextKey::AddressBookConnect => "Connect",
        TextKey::AddressBookDeleteContact => "Delete contact",
        TextKey::AddressBookRecentTitle => "Recent",
        TextKey::AddressBookRecentDescription => "Recent connection addresses",
        TextKey::AddressBookClearHistory => "Clear history",
        TextKey::AddressBookHistoryEmpty => "History is empty",
        TextKey::AddressBookHistoryEmptyHint => "Recent connections will appear here.",
        TextKey::AddressBookHistoryNotFound => "Nothing found in history",
        TextKey::AddressBookSelectAddress => "Use address",
        TextKey::AddressBookRemoveFromHistory => "Remove from history",
        TextKey::AddressBookEditing => "Editing",
        TextKey::AddressBookNewContact => "New contact",
        TextKey::AddressBookNameAndIdRequired => "Name and ID are required",
        TextKey::AddressBookCloseForm => "Close form",
        TextKey::AddressBookDeviceNamePlaceholder => "Device name",
        TextKey::AddressBookGroupPathPlaceholder => "Group / path",
        TextKey::AddressBookTagsPlaceholder => "Tags separated by commas",
        TextKey::AddressBookNotePlaceholder => "Note",
        TextKey::AddressBookGroups => "Groups",
        TextKey::AddressBookTags => "Tags",
        TextKey::AddressBookSave => "Save",
        TextKey::AddressBookAdd => "Add",
        TextKey::AddressBookClear => "Clear",
        TextKey::AddressBookAllContacts => "All contacts",
        TextKey::AddressBookFavorites => "Favorites",
        TextKey::AddressBookRecent => "Recent",
        TextKey::AddressBookRecentContacts => "Recent contacts",
        TextKey::AddressBookGroupContacts => "Group contacts",
        TextKey::AddressBookTaggedContacts => "Tagged contacts",
        TextKey::AddressBookAllShort => "All",
        TextKey::AddressBookResetFilter => "Reset address book filter",
        TextKey::AddressBookSearchPlaceholder => "Search by name, ID, group, tag, or note",
        TextKey::AddressBookShownTotalTemplate => "{} shown · {} total",
        TextKey::AddressBookSync => "Sync address book",
        TextKey::AddressBookSyncRestored => {
            "Sign-in was restored. Run sync to refresh cloud contacts."
        }
        TextKey::AddressBookSyncAvailable => {
            "Local contacts are always available; cloud contacts sync manually."
        }
        TextKey::AddressBookSyncEnabled => "Sync enabled",
        TextKey::AddressBookRefreshEntitlements => "Refresh account entitlements",
        TextKey::AddressBookSignOutCloud => "Sign out of cloud address book",
        TextKey::AddressBookSignIn => "Sign in",
        TextKey::AddressBookSigningIn => "Signing in…",
        TextKey::AddressBookYandex => "Yandex",
        TextKey::AddressBookWaitingYandex => "Waiting for Yandex…",
        TextKey::AddressBookCancel => "Cancel",
        TextKey::AddressBookLocalWorks => {
            "The local address book works without sign-in. Authorization is needed only for cloud sync."
        }
        TextKey::AddressBookLocalTitle => "Local address book",
        TextKey::AddressBookLoginPlaceholder => "Login or e-mail",
        TextKey::AddressBookPasswordPlaceholder => "Password or token",
        TextKey::AddressBookContactDetails => "Contact details",
        TextKey::AddressBookQuickActions => "Quick actions without opening the form",
        TextKey::AddressBookHideDetails => "Hide details",
        TextKey::AddressBookCopyId => "Copy ID",
        TextKey::AddressBookUseAddress => "Use address",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionaries_cover_core_settings_and_update_strings() {
        let keys = [
            TextKey::LanguageTitle,
            TextKey::LanguageDescription,
            TextKey::LanguageSaved,
            TextKey::UpdatesTitle,
            TextKey::UpdatesDescription,
            TextKey::UpdatesDisabledHint,
            TextKey::UpdatesManifestPlaceholder,
            TextKey::UpdatesGithubPlaceholder,
            TextKey::UpdatesChannelNotConfigured,
            TextKey::UpdatesCurrentVersion,
            TextKey::UpdatesCheck,
            TextKey::UpdatesChecking,
            TextKey::UpdatesUpToDate,
            TextKey::UpdatesCheckAgain,
            TextKey::UpdatesAvailable,
            TextKey::UpdatesDownloadAndVerify,
            TextKey::UpdatesDownloading,
            TextKey::UpdatesReadyToInstall,
            TextKey::UpdatesInstall,
            TextKey::UpdatesRetry,
            TextKey::UpdateChannelSaved,
            TextKey::UpdateManifestUrlSaved,
            TextKey::UpdateGithubRepoSaved,
            TextKey::NavHome,
            TextKey::NavAddressBook,
            TextKey::NavSettings,
            TextKey::SettingsSectionSecurity,
            TextKey::SettingsSectionGeneral,
            TextKey::SettingsSectionConnection,
            TextKey::SettingsHintSecurity,
            TextKey::SettingsHintGeneral,
            TextKey::SettingsHintConnection,
            TextKey::SettingsSectionsTitle,
            TextKey::LanguageSystem,
            TextKey::LanguageRussian,
            TextKey::LanguageEnglish,
            TextKey::LanguageSystemHint,
            TextKey::LanguageRussianHint,
            TextKey::LanguageEnglishHint,
            TextKey::UpdateChannelDisabled,
            TextKey::UpdateChannelManifestUrl,
            TextKey::UpdateChannelGithubRelease,
            TextKey::UpdateChannelDisabledHint,
            TextKey::UpdateChannelManifestUrlHint,
            TextKey::UpdateChannelGithubReleaseHint,
            TextKey::HomeCredentialTitle,
            TextKey::HomeCredentialSubtitlePrefix,
            TextKey::HomeRemotePasswordPlaceholder,
            TextKey::HomeRememberPassword,
            TextKey::HomeRememberPasswordHint,
            TextKey::HomeCancel,
            TextKey::HomeConnect,
            TextKey::HomeStopReceiving,
            TextKey::HomeEnableAccess,
            TextKey::HomeHide,
            TextKey::HomeShow,
            TextKey::HomeThisWorkspace,
            TextKey::HomeCopy,
            TextKey::HomeOneTimePassword,
            TextKey::HomeRefreshNow,
            TextKey::HomeRemoteAddressPlaceholder,
            TextKey::HomeFavorites,
            TextKey::HomeRecentSessions,
            TextKey::HomeRecentEmptyTitle,
            TextKey::HomeRecentEmptyHint,
            TextKey::HomeRemoteDevice,
            TextKey::SettingsTitle,
            TextKey::SettingsSubtitle,
            TextKey::SettingsPermanentPassword,
            TextKey::SettingsPermanentPasswordDescription,
            TextKey::SettingsPermanentPasswordPlaceholder,
            TextKey::SettingsTemporaryPasswordRotates,
            TextKey::SettingsDelete,
            TextKey::SettingsSave,
            TextKey::SettingsIncomingTitle,
            TextKey::SettingsIncomingDescription,
            TextKey::SettingsAlwaysAskConfirmation,
            TextKey::SettingsAlwaysAskConfirmationHint,
            TextKey::SettingsAccessAutoTitle,
            TextKey::SettingsAccessAutoHint,
            TextKey::SettingsPermissionsTitle,
            TextKey::SettingsPermissionsDescription,
            TextKey::SettingsKeyboardMouse,
            TextKey::SettingsKeyboardMouseHint,
            TextKey::SettingsSharedClipboard,
            TextKey::SettingsSharedClipboardHint,
            TextKey::SettingsOutgoingTitle,
            TextKey::SettingsOutgoingDescription,
            TextKey::SettingsImageQuality,
            TextKey::SettingsQualityHint,
            TextKey::QualitySmooth,
            TextKey::QualityBalanced,
            TextKey::QualitySharp,
            TextKey::SettingsStreamingMode,
            TextKey::StreamingModeSupportHint,
            TextKey::StreamingModeInteractiveHint,
            TextKey::StreamingModeGameHint,
            TextKey::SettingsFsrUpscale,
            TextKey::SettingsFsrHint,
            TextKey::SettingsPlayRemoteAudio,
            TextKey::SettingsPlayRemoteAudioHint,
            TextKey::SettingsAppBehaviorTitle,
            TextKey::SettingsAppBehaviorDescription,
            TextKey::SettingsLaunchOnStartup,
            TextKey::SettingsLaunchOnStartupHint,
            TextKey::SettingsShowStartMenuShortcut,
            TextKey::SettingsShowStartMenuShortcutHint,
            TextKey::SettingsKeepTaskbarIcon,
            TextKey::SettingsKeepTaskbarIconHintOn,
            TextKey::SettingsKeepTaskbarIconHintOff,
            TextKey::SettingsSmartAgentTitle,
            TextKey::SettingsSmartAgentDescription,
            TextKey::SettingsSmartAgentAvailable,
            TextKey::SettingsSmartAgentEnable,
            TextKey::SettingsSmartAgentServiceKeyPlaceholder,
            TextKey::SettingsSmartAgentIdleHint,
            TextKey::SettingsCompatibilityTitle,
            TextKey::SettingsCompatibilityCustom,
            TextKey::SettingsCompatibilityDefault,
            TextKey::SettingsCompatibilityHide,
            TextKey::SettingsCompatibilityShow,
            TextKey::SettingsCompatibilityDiscover,
            TextKey::SettingsCompatibilityDiscovering,
            TextKey::SettingsCompatibilityDiscoveryHint,
            TextKey::SettingsCompatibilityEmptyFieldsHint,
            TextKey::SettingsReset,
            TextKey::AboutTitle,
            TextKey::AboutSubtitle,
            TextKey::AboutAuthor,
            TextKey::AboutVersion,
            TextKey::AboutGithub,
            TextKey::AboutHabr,
            TextKey::AboutContact,
            TextKey::AboutDesk,
            TextKey::AboutDeskDescription,
            TextKey::AboutCheckUpdates,
            TextKey::AboutClose,
            TextKey::AboutCopyEmail,
            TextKey::AddressBookTitle,
            TextKey::AddressBookSubtitle,
            TextKey::AddressBookNoGroup,
            TextKey::AddressBookDeviceId,
            TextKey::AddressBookLocalCloudDevices,
            TextKey::AddressBookHideContactForm,
            TextKey::AddressBookAddNewContact,
            TextKey::AddressBookNoSavedDevices,
            TextKey::AddressBookNoSavedDevicesHint,
            TextKey::AddressBookContactsNotFound,
            TextKey::AddressBookTryChangeSearch,
            TextKey::AddressBookRemoveFromFavorites,
            TextKey::AddressBookAddToFavorites,
            TextKey::AddressBookShowDetails,
            TextKey::AddressBookEditContact,
            TextKey::AddressBookConnect,
            TextKey::AddressBookDeleteContact,
            TextKey::AddressBookRecentTitle,
            TextKey::AddressBookRecentDescription,
            TextKey::AddressBookClearHistory,
            TextKey::AddressBookHistoryEmpty,
            TextKey::AddressBookHistoryEmptyHint,
            TextKey::AddressBookHistoryNotFound,
            TextKey::AddressBookSelectAddress,
            TextKey::AddressBookRemoveFromHistory,
            TextKey::AddressBookEditing,
            TextKey::AddressBookNewContact,
            TextKey::AddressBookNameAndIdRequired,
            TextKey::AddressBookCloseForm,
            TextKey::AddressBookDeviceNamePlaceholder,
            TextKey::AddressBookGroupPathPlaceholder,
            TextKey::AddressBookTagsPlaceholder,
            TextKey::AddressBookNotePlaceholder,
            TextKey::AddressBookGroups,
            TextKey::AddressBookTags,
            TextKey::AddressBookSave,
            TextKey::AddressBookAdd,
            TextKey::AddressBookClear,
            TextKey::AddressBookAllContacts,
            TextKey::AddressBookFavorites,
            TextKey::AddressBookRecent,
            TextKey::AddressBookRecentContacts,
            TextKey::AddressBookGroupContacts,
            TextKey::AddressBookTaggedContacts,
            TextKey::AddressBookAllShort,
            TextKey::AddressBookResetFilter,
            TextKey::AddressBookSearchPlaceholder,
            TextKey::AddressBookShownTotalTemplate,
            TextKey::AddressBookSync,
            TextKey::AddressBookSyncRestored,
            TextKey::AddressBookSyncAvailable,
            TextKey::AddressBookSyncEnabled,
            TextKey::AddressBookRefreshEntitlements,
            TextKey::AddressBookSignOutCloud,
            TextKey::AddressBookSignIn,
            TextKey::AddressBookSigningIn,
            TextKey::AddressBookYandex,
            TextKey::AddressBookWaitingYandex,
            TextKey::AddressBookCancel,
            TextKey::AddressBookLocalWorks,
            TextKey::AddressBookLocalTitle,
            TextKey::AddressBookLoginPlaceholder,
            TextKey::AddressBookPasswordPlaceholder,
            TextKey::AddressBookContactDetails,
            TextKey::AddressBookQuickActions,
            TextKey::AddressBookHideDetails,
            TextKey::AddressBookCopyId,
            TextKey::AddressBookUseAddress,
        ];

        for key in keys {
            assert!(!tr(UiLanguage::Russian, key).trim().is_empty());
            assert!(!tr(UiLanguage::English, key).trim().is_empty());
        }
    }

    #[test]
    fn explicit_language_preference_overrides_system_locale() {
        assert_eq!(
            UiLanguage::from_preference(LanguagePreference::Russian),
            UiLanguage::Russian
        );
        assert_eq!(
            UiLanguage::from_preference(LanguagePreference::English),
            UiLanguage::English
        );
    }

    #[test]
    fn locale_detection_recognizes_russian_system_locales() {
        assert_eq!(language_from_locale("ru-RU"), Some(UiLanguage::Russian));
        assert_eq!(
            language_from_locale("ru_RU.UTF-8"),
            Some(UiLanguage::Russian)
        );
        assert_eq!(language_from_locale("en-US"), Some(UiLanguage::English));
        assert_eq!(language_from_locale(""), None);
    }
}
