// Claude Vision — quotas : indicateur de la barre du haut.
//
// Lit seulement ~/.cache/claude-vision/tray.json, publié toutes les 5 s par
// l'application Claude Vision et supprimé à sa fermeture. N'appelle aucune API
// et ne lit aucun jeton. L'indicateur est masqué quand le fichier manque, est
// illisible ou date de plus de 20 s.

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import {
    accountErrorMarkup,
    accountMenuMarkup,
    isFresh,
    panelMarkup,
    parseState,
    sessionsMarkup,
} from './format.js';

const POLL_SECONDS = 3;

// Élément de menu purement informatif : ni clic, ni survol, ni focus.
function infoItem() {
    const item = new PopupMenu.PopupMenuItem('', {
        activate: false,
        hover: false,
        can_focus: false,
    });
    item.label.add_style_class_name('claude-vision-tray-menu-label');
    return item;
}

function setMarkup(label, markup) {
    label.clutter_text.set_markup(markup);
}

const ClaudeVisionIndicator = GObject.registerClass(
class ClaudeVisionIndicator extends PanelMenu.Button {
    _init() {
        super._init(0.5, 'Claude Vision', false);

        this._label = new St.Label({
            style_class: 'claude-vision-tray-label',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this.add_child(this._label);

        this._rows = [];      // [{main, error}] par compte
        this._sessionsSep = null;
        this._sessionsItem = null;
    }

    _rebuildMenu(count) {
        this.menu.removeAll();
        this._rows = [];
        for (let i = 0; i < count; i++) {
            const main = infoItem();
            const error = infoItem();
            this.menu.addMenuItem(main);
            this.menu.addMenuItem(error);
            this._rows.push({main, error});
        }
        this._sessionsSep = new PopupMenu.PopupSeparatorMenuItem();
        this._sessionsItem = infoItem();
        this.menu.addMenuItem(this._sessionsSep);
        this.menu.addMenuItem(this._sessionsItem);
    }

    update(state) {
        setMarkup(this._label, panelMarkup(state));

        if (this._rows.length !== state.accounts.length || !this._sessionsItem)
            this._rebuildMenu(state.accounts.length);

        const now = GLib.DateTime.new_now_utc();
        state.accounts.forEach((a, i) => {
            const row = this._rows[i];
            setMarkup(row.main.label, accountMenuMarkup(a, now, state.lang));
            const err = accountErrorMarkup(a, state.lang);
            row.error.visible = err !== null;
            if (err !== null)
                setMarkup(row.error.label, err);
        });

        const sessions = sessionsMarkup(state);
        this._sessionsSep.visible = sessions !== null && state.accounts.length > 0;
        this._sessionsItem.visible = sessions !== null;
        if (sessions !== null)
            setMarkup(this._sessionsItem.label, sessions);
    }
});

export default class ClaudeVisionTrayExtension extends Extension {
    enable() {
        const path = GLib.build_filenamev(
            [GLib.get_user_cache_dir(), 'claude-vision', 'tray.json']);
        this._file = Gio.File.new_for_path(path);
        this._cancellable = new Gio.Cancellable();
        this._state = null;
        this._reading = false;

        this._indicator = new ClaudeVisionIndicator();
        Main.panel.addToStatusArea(this.uuid, this._indicator, 0, 'right');
        this._setShown(false);

        // Toujours le premier élément à gauche du groupe de droite, comme s'il
        // se chargeait en dernier : on se replace dès qu'un autre élément
        // s'ajoute, puis une fois que toutes les extensions sont chargées.
        this._box = Main.panel._rightBox;
        this._childAddedId = this._box.connect('child-added', () => this._keepFirst());
        this._idleId = GLib.idle_add(GLib.PRIORITY_LOW, () => {
            this._idleId = 0;
            this._keepFirst();
            return GLib.SOURCE_REMOVE;
        });
        this._keepFirst();

        this._poll();
        this._timerId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, POLL_SECONDS, () => {
            this._poll();
            return GLib.SOURCE_CONTINUE;
        });
    }

    disable() {
        if (this._timerId) {
            GLib.Source.remove(this._timerId);
            this._timerId = 0;
        }
        if (this._idleId) {
            GLib.Source.remove(this._idleId);
            this._idleId = 0;
        }
        if (this._box && this._childAddedId) {
            this._box.disconnect(this._childAddedId);
            this._childAddedId = 0;
        }
        this._box = null;
        this._cancellable?.cancel();
        this._cancellable = null;
        this._indicator?.destroy();
        this._indicator = null;
        this._file = null;
        this._state = null;
        this._reading = false;
    }

    // Replace le conteneur de l'indicateur en tête du groupe de droite.
    _keepFirst() {
        const box = this._box;
        const container = this._indicator?.container;
        if (!box || !container || container.get_parent() !== box)
            return;
        if (box.get_child_at_index(0) !== container)
            box.set_child_at_index(container, 0);
    }

    _poll() {
        if (this._reading || !this._file)
            return;
        this._reading = true;
        const cancellable = this._cancellable;
        this._file.load_contents_async(cancellable, (file, res) => {
            let bytes = null;
            let readOk = true;
            try {
                [, bytes] = file.load_contents_finish(res);
            } catch {
                // Fichier absent (app fermée), illisible ou lecture annulée.
                readOk = false;
            }
            // Extension désactivée (ou réactivée) entre-temps : ignorer.
            if (cancellable.is_cancelled() || cancellable !== this._cancellable)
                return;
            this._reading = false;

            if (!readOk) {
                this._state = null;
            } else {
                try {
                    this._state = parseState(bytes);
                } catch {
                    // JSON partiel ou inattendu : on garde le dernier état
                    // valide ; son âge décidera s'il reste affiché.
                }
            }
            this._refresh();
        });
    }

    _refresh() {
        if (!this._indicator)
            return;
        const shown = isFresh(this._state, Date.now());
        if (shown) {
            try {
                this._indicator.update(this._state);
            } catch (e) {
                console.error(`[claude-vision-tray] mise à jour impossible : ${e}`);
                this._setShown(false);
                return;
            }
        }
        this._setShown(shown);
    }

    _setShown(shown) {
        const ind = this._indicator;
        if (!shown && ind.menu.isOpen)
            ind.menu.close();
        ind.visible = shown;
        ind.container.visible = shown;
    }
}
