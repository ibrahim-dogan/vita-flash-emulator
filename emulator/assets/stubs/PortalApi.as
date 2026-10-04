package {
    import flash.display.Sprite;

    /**
     * Stand-in for the portal APIs games load at startup: Kongregate's
     * API_AS3_Local.swf / API_AS3.swf and MindJolt's api_as3_local.swf. The
     * real files can't be fetched any more, and games that call the API
     * without checking it loaded then stop on a null reference (Mega Drill's
     * second Continue button never gets its click listener). RuffleVita
     * serves this instead; see navigator.rs.
     */
    public dynamic class PortalApi extends Sprite {
        public var services:AnyStub = new AnyStub();
        public var user:AnyStub = new AnyStub();
        public var stats:AnyStub = new AnyStub();
        public var mtx:AnyStub = new AnyStub();
        public var sharedContent:AnyStub = new AnyStub();
        public var images:AnyStub = new AnyStub();
        public var chat:AnyStub = new AnyStub();
        public var api:AnyStub = new AnyStub();
        // MindJolt: MindJoltAPI.service.connect(callback), .submitScore(...).
        public var service:AnyStub = new AnyStub();
        public var loaded:Boolean = true;
        public var connected:Boolean = true;
    }
}
