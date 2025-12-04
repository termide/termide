use super::Panel;

/// Группа панелей в аккордеоне (вертикальный стек)
pub struct PanelGroup {
    panels: Vec<Box<dyn Panel>>,
    expanded_index: usize,  // Индекс развёрнутой панели
    pub width: Option<u16>, // Ширина в символах (None = авто-распределение)
}

impl PanelGroup {
    /// Создать новую группу с одной панелью
    pub fn new(panel: Box<dyn Panel>) -> Self {
        Self {
            panels: vec![panel],
            expanded_index: 0,
            width: None, // По умолчанию авто-распределение
        }
    }

    /// Добавить панель в группу
    pub fn add_panel(&mut self, panel: Box<dyn Panel>) {
        self.panels.push(panel);
    }

    /// Удалить панель из группы
    pub fn remove_panel(&mut self, index: usize) -> Option<Box<dyn Panel>> {
        if index >= self.panels.len() {
            return None;
        }

        let panel = self.panels.remove(index);

        // Скорректировать expanded_index
        if self.panels.is_empty() {
            // Группа пуста - сбросить индекс для консистентности
            self.expanded_index = 0;
        } else if self.expanded_index >= self.panels.len() {
            self.expanded_index = self.panels.len() - 1;
        }

        Some(panel)
    }

    /// Установить развёрнутую панель
    pub fn set_expanded(&mut self, index: usize) {
        if index < self.panels.len() {
            self.expanded_index = index;
        }
    }

    /// Получить индекс развёрнутой панели
    pub fn expanded_index(&self) -> usize {
        self.expanded_index
    }

    /// Переключиться на следующую панель в группе
    pub fn next_panel(&mut self) {
        if !self.panels.is_empty() {
            self.expanded_index = (self.expanded_index + 1) % self.panels.len();
        }
    }

    /// Переключиться на предыдущую панель в группе
    pub fn prev_panel(&mut self) {
        if !self.panels.is_empty() {
            self.expanded_index = if self.expanded_index == 0 {
                self.panels.len() - 1
            } else {
                self.expanded_index - 1
            };
        }
    }

    /// Получить количество панелей в группе
    pub fn len(&self) -> usize {
        self.panels.len()
    }

    /// Проверить, пуста ли группа
    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }

    /// Получить мутабельную ссылку на панели
    pub fn panels_mut(&mut self) -> &mut Vec<Box<dyn Panel>> {
        &mut self.panels
    }

    /// Получить ссылку на панели
    pub fn panels(&self) -> &Vec<Box<dyn Panel>> {
        &self.panels
    }

    /// Получить мутабельную ссылку на развёрнутую панель
    pub fn expanded_panel_mut(&mut self) -> Option<&mut Box<dyn Panel>> {
        self.panels.get_mut(self.expanded_index)
    }

    /// Переместить панель вверх (поменять местами с предыдущей)
    pub fn move_panel_up(&mut self, index: usize) -> anyhow::Result<()> {
        // Проверить границы
        if index == 0 || self.panels.is_empty() {
            return Ok(()); // Уже в верхней позиции или пустая группа
        }
        if index >= self.panels.len() {
            return Err(anyhow::anyhow!("Panel index out of bounds"));
        }

        // Поменять панели местами
        self.panels.swap(index - 1, index);

        // Обновить expanded_index
        if self.expanded_index == index {
            self.expanded_index = index - 1;
        } else if self.expanded_index == index - 1 {
            self.expanded_index = index;
        }

        Ok(())
    }

    /// Переместить панель вниз (поменять местами со следующей)
    pub fn move_panel_down(&mut self, index: usize) -> anyhow::Result<()> {
        // Проверить границы
        if self.panels.is_empty() {
            return Ok(()); // Пустая группа
        }
        if index >= self.panels.len() - 1 {
            return Ok(()); // Уже в нижней позиции
        }

        // Поменять панели местами
        self.panels.swap(index, index + 1);

        // Обновить expanded_index
        if self.expanded_index == index {
            self.expanded_index = index + 1;
        } else if self.expanded_index == index + 1 {
            self.expanded_index = index;
        }

        Ok(())
    }

    /// Забрать все панели из группы (опустошить группу)
    pub fn take_panels(self) -> Vec<Box<dyn Panel>> {
        self.panels
    }
}
