package {
    import flash.utils.Proxy;
    import flash.utils.flash_proxy;

    /**
     * One part of a portal API (Kongregate's services, stats, mtx, ...,
     * MindJolt's service). Every property is another stub and every call
     * succeeds, so whatever a game calls on the API is a harmless no-op. The
     * few queries games branch on answer as a signed-out guest.
     */
    public dynamic class AnyStub extends Proxy {
        private var children:Object = {};

        private static function key(name:*):String {
            return name is QName ? QName(name).localName : String(name);
        }

        override flash_proxy function getProperty(name:*):* {
            var k:String = key(name);
            if (!(k in children)) {
                children[k] = new AnyStub();
            }
            return children[k];
        }

        override flash_proxy function setProperty(name:*, value:*):void {
            children[key(name)] = value;
        }

        override flash_proxy function hasProperty(name:*):Boolean {
            return true;
        }

        override flash_proxy function deleteProperty(name:*):Boolean {
            return delete children[key(name)];
        }

        override flash_proxy function callProperty(name:*, ...rest):* {
            switch (key(name)) {
                case "isGuest":
                    return true;
                case "getUsername":
                case "getName":
                    return "Guest";
                case "getUserId":
                case "getUserID":
                    return 0;
                case "getGameAuthToken":
                    return "";
                case "connect":
                case "isConnected":
                    return true;
                case "hasKongPlus":
                case "isKongregate":
                    return false;
            }
            return undefined;
        }
    }
}
